use std::time::Instant;

use crate::api::schema::{
    PaneBindRuntimeParams, PaneReportRuntimeParams, PaneTarget, PaneUnbindRuntimeParams,
    ResponseResult, RuntimeBinding, RuntimeState,
};
use crate::app::App;
use crate::terminal::TerminalId;

use super::responses::{encode_error, encode_success};

impl App {
    fn runtime_binding_target(
        &self,
        pane_id: &str,
    ) -> Option<(usize, crate::layout::PaneId, TerminalId)> {
        let (ws_idx, id) = self.parse_current_public_pane_id(pane_id)?;
        Some((
            ws_idx,
            id,
            self.state.workspaces[ws_idx].terminal_id(id)?.clone(),
        ))
    }

    pub(super) fn handle_pane_bind_runtime(
        &mut self,
        id: String,
        params: PaneBindRuntimeParams,
    ) -> String {
        let Some((ws_idx, pane_id, terminal_id)) = self.runtime_binding_target(&params.pane_id)
        else {
            return encode_error(id, "pane_not_found", "exact pane identity not found");
        };
        if params.binding_id.is_empty()
            || params.binding_id.len() > 512
            || params.binding_id.chars().any(char::is_control)
        {
            return encode_error(id, "invalid_runtime_binding", "invalid binding ID");
        }
        if let Err(message) = params.attachment.validate() {
            return encode_error(id, "invalid_runtime_attachment", message);
        }
        let terminal = &self.state.terminals[&terminal_id];
        let current = terminal.runtime_binding.as_ref();
        let owner = self.state.runtime_binding_owners.get(&params.binding_id);
        if current.is_some_and(|binding| {
            binding.binding_id != params.binding_id || binding.attachment != params.attachment
        }) || owner.is_some_and(|binding| {
            binding.pane_id != params.pane_id || binding.attachment != params.attachment
        }) {
            return encode_error(
                id,
                "runtime_binding_conflict",
                "binding identity is immutable; explicitly unbind the exact current binding",
            );
        }
        if current.is_some() {
            return encode_success(
                id,
                ResponseResult::PaneRuntime {
                    applied: Some(true),
                    binding: terminal.runtime_binding_snapshot(Instant::now()),
                },
            );
        }
        let binding = RuntimeBinding {
            pane_id: params.pane_id,
            binding_id: params.binding_id,
            attachment: params.attachment,
            seq: owner.and_then(|binding| binding.seq),
            state: RuntimeState::Unknown,
            label: None,
            ttl_ms: crate::runtime_attachment::DEFAULT_TTL_MS,
            fresh: false,
        };
        self.state
            .runtime_binding_owners
            .insert(binding.binding_id.clone(), binding.clone());
        if let Some(update) = self.state.update_terminal_state(pane_id, |terminal| {
            Some(terminal.mutate_runtime_binding(Some(binding.clone()), None))
        }) {
            self.emit_pane_state_update(&update);
        }
        self.schedule_session_save();
        self.emit_pane_updated(ws_idx, pane_id);
        encode_success(
            id,
            ResponseResult::PaneRuntime {
                applied: Some(true),
                binding: Some(binding),
            },
        )
    }

    pub(super) fn handle_pane_get_runtime(&mut self, id: String, params: PaneTarget) -> String {
        let Some((_, _, terminal_id)) = self.runtime_binding_target(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "exact pane identity not found");
        };
        encode_success(
            id,
            ResponseResult::PaneRuntime {
                applied: None,
                binding: self.state.terminals[&terminal_id]
                    .runtime_binding_snapshot(Instant::now()),
            },
        )
    }

    pub(super) fn handle_pane_report_runtime(
        &mut self,
        id: String,
        params: PaneReportRuntimeParams,
    ) -> String {
        let Some((ws_idx, pane_id, terminal_id)) = self.runtime_binding_target(&params.pane_id)
        else {
            return encode_error(id, "pane_not_found", "exact pane identity not found");
        };
        let ttl_ms = params
            .ttl_ms
            .unwrap_or(crate::runtime_attachment::DEFAULT_TTL_MS);
        if !(1..=86_400_000).contains(&ttl_ms)
            || params
                .label
                .as_ref()
                .is_some_and(|label| label.len() > 512 || label.chars().any(char::is_control))
        {
            return encode_error(
                id,
                "invalid_runtime_report",
                "invalid observation TTL or label",
            );
        }
        let terminal = &self.state.terminals[&terminal_id];
        let Some(mut binding) = terminal.runtime_binding.clone().filter(|binding| {
            binding.binding_id == params.binding_id
                && binding.pane_id == params.pane_id
                && binding.seq.is_none_or(|seq| params.seq > seq)
        }) else {
            return encode_success(
                id,
                ResponseResult::PaneRuntime {
                    applied: Some(false),
                    binding: terminal.runtime_binding_snapshot(Instant::now()),
                },
            );
        };
        let now = Instant::now();
        let previous_runtime_state = binding.state;
        // An expired Done is a new completion; a Done reset by cold restore is not.
        let previous_runtime_done = terminal.runtime_restored_done
            || (previous_runtime_state == RuntimeState::Done
                && terminal
                    .runtime_expiry()
                    .is_some_and(|deadline| now < deadline));
        let previous_detect_state = terminal.state;
        binding.seq = Some(params.seq);
        binding.state = params.state;
        binding.label = params.label;
        binding.ttl_ms = ttl_ms;
        binding.fresh = true;
        self.state
            .runtime_binding_owners
            .insert(binding.binding_id.clone(), binding.clone());
        let update = self.state.update_terminal_state(pane_id, |terminal| {
            Some(terminal.mutate_runtime_binding(Some(binding.clone()), Some(now)))
        });
        let same_detect_transition = previous_runtime_state != binding.state
            && previous_detect_state == binding.state.detect();
        let detect_changed = update
            .as_ref()
            .is_some_and(|update| update.previous_state != update.state);
        // Working/Blocked -> Done already completes through the generic Idle edge.
        let generic_completion = update.as_ref().is_some_and(|update| {
            update.state == crate::detect::AgentState::Idle
                && matches!(
                    update.previous_state,
                    crate::detect::AgentState::Working | crate::detect::AgentState::Blocked
                )
        });
        let runtime_completion =
            binding.state == RuntimeState::Done && !previous_runtime_done && !generic_completion;
        if same_detect_transition || runtime_completion {
            if !detect_changed {
                self.state.next_agent_state_change_seq += 1;
            }
            if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
                terminal.last_agent_state_change_seq = Some(self.state.next_agent_state_change_seq);
                terminal.last_agent_completion_seq =
                    runtime_completion.then_some(self.state.next_agent_state_change_seq);
            }
            if runtime_completion {
                self.state.publish_runtime_completion(ws_idx, pane_id);
            }
        }
        if let Some(update) = &update {
            self.emit_pane_state_update(update);
        }
        if (same_detect_transition || runtime_completion) && update.is_none() {
            if let Some(pane) = self.pane_info(ws_idx, pane_id) {
                self.emit_event(crate::api::schema::EventEnvelope {
                    event: crate::api::schema::EventKind::PaneAgentStatusChanged,
                    data: crate::api::schema::EventData::PaneAgentStatusChanged {
                        pane_id: binding.pane_id.clone(),
                        workspace_id: pane.workspace_id,
                        agent_status: pane.agent_status,
                        agent: pane.agent,
                        title: pane.title,
                        display_agent: pane.display_agent,
                        state_labels: pane.state_labels,
                    },
                });
            }
        }
        self.sync_agent_metadata_deadline();
        self.schedule_session_save();
        self.emit_pane_updated(ws_idx, pane_id);
        encode_success(
            id,
            ResponseResult::PaneRuntime {
                applied: Some(true),
                binding: Some(binding),
            },
        )
    }

    pub(super) fn handle_pane_unbind_runtime(
        &mut self,
        id: String,
        params: PaneUnbindRuntimeParams,
    ) -> String {
        let Some((ws_idx, pane_id, terminal_id)) = self.runtime_binding_target(&params.pane_id)
        else {
            return encode_error(id, "pane_not_found", "exact pane identity not found");
        };
        let terminal = &self.state.terminals[&terminal_id];
        let exact = terminal
            .runtime_binding
            .as_ref()
            .is_some_and(|binding| binding.binding_id == params.binding_id);
        let repeat = terminal.runtime_binding.is_none()
            && self
                .state
                .runtime_binding_owners
                .get(&params.binding_id)
                .is_some_and(|binding| binding.pane_id == params.pane_id);
        if !exact && !repeat {
            return encode_success(
                id,
                ResponseResult::PaneRuntime {
                    applied: Some(false),
                    binding: terminal.runtime_binding_snapshot(Instant::now()),
                },
            );
        }
        let pruned = self.state.prune_runtime_binding_owners();
        if exact {
            if let Some(update) = self.state.update_terminal_state(pane_id, |terminal| {
                Some(terminal.mutate_runtime_binding(None, None))
            }) {
                self.emit_pane_state_update(&update);
            }
            self.sync_agent_metadata_deadline();
            self.emit_pane_updated(ws_idx, pane_id);
        }
        if exact || pruned {
            self.schedule_session_save();
        }
        encode_success(
            id,
            ResponseResult::PaneRuntime {
                applied: Some(true),
                binding: None,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{RuntimeAttachment, SuccessResponse};

    fn app() -> App {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            rx,
            crate::api::EventHub::default(),
        );
        app.state = crate::app::AppState::test_with_adversarial_identity_state();
        app.state.ensure_test_terminals();
        app
    }

    fn attachment() -> RuntimeAttachment {
        let root = std::env::temp_dir();
        let location = root.join("herdr-native-project").display().to_string();
        RuntimeAttachment {
            provider: "shuvcode".into(),
            home_id: "home-a".into(),
            session_id: "ses_a".into(),
            location: location.clone(),
            host_id: "local".into(),
            attach_argv: [
                root.join("shuvcode").display().to_string(),
                "supervisor".into(),
                "attach".into(),
                "--home".into(),
                root.join("herdr-native-home").display().to_string(),
                "--home-id".into(),
                "home-a".into(),
                "--session".into(),
                "ses_a".into(),
                "--location".into(),
                location,
            ]
            .into(),
        }
    }

    #[test]
    fn runtime_attachment_identity_sequences_and_unbind_preserve_topology() {
        let mut app = app();
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        let other = app
            .public_pane_id(0, app.state.workspaces[0].tabs[1].root_pane)
            .unwrap();
        let bind = PaneBindRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
            attachment: attachment(),
        };
        assert!(app
            .handle_pane_bind_runtime("bind".into(), bind.clone())
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_bind_runtime("retry".into(), bind.clone())
            .contains("\"applied\":true"));
        let mut conflict = bind.clone();
        conflict.pane_id = other;
        assert!(app
            .handle_pane_bind_runtime("collision".into(), conflict)
            .contains("runtime_binding_conflict"));
        let report = PaneReportRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
            seq: 10,
            state: RuntimeState::Working,
            label: Some("native work".into()),
            ttl_ms: Some(5_000),
        };
        assert!(app
            .handle_pane_report_runtime("report".into(), report.clone())
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_report_runtime("stale".into(), report.clone())
            .contains("\"applied\":false"));
        let mut wrong = report.clone();
        wrong.binding_id = "binding-b".into();
        wrong.seq = 11;
        assert!(app
            .handle_pane_report_runtime("foreign".into(), wrong)
            .contains("\"applied\":false"));
        let unbind = PaneUnbindRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
        };
        assert!(app
            .handle_pane_unbind_runtime("unbind".into(), unbind.clone())
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_unbind_runtime("retry-unbind".into(), unbind)
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_bind_runtime("rebind".into(), bind)
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_report_runtime("old-after-rebind".into(), report)
            .contains("\"applied\":false"));
        let response: SuccessResponse = serde_json::from_str(
            &app.handle_pane_get_runtime("get".into(), PaneTarget { pane_id }),
        )
        .unwrap();
        let ResponseResult::PaneRuntime {
            binding: Some(binding),
            ..
        } = response.result
        else {
            panic!("binding missing")
        };
        assert_eq!(binding.seq, Some(10));
        assert_eq!(binding.state, RuntimeState::Unknown);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_owner_is_pruned_when_owning_pane_closes() {
        let mut app = app();
        let pane_id = |app: &App, tab: usize| {
            app.public_pane_id(0, app.state.workspaces[0].tabs[tab].root_pane)
                .unwrap()
        };
        let bound_pane = pane_id(&app, 0);
        let unbound_pane = pane_id(&app, 1);
        let survivor_pane = pane_id(&app, 2);
        let bind = |pane_id: &str, binding_id: &str| PaneBindRuntimeParams {
            pane_id: pane_id.into(),
            binding_id: binding_id.into(),
            attachment: attachment(),
        };
        for (pane, binding_id) in [(&bound_pane, "bound"), (&unbound_pane, "unbound")] {
            assert!(app
                .handle_pane_bind_runtime("bind".into(), bind(pane, binding_id))
                .contains("\"applied\":true"));
        }
        assert!(app
            .handle_pane_unbind_runtime(
                "unbind".into(),
                PaneUnbindRuntimeParams {
                    pane_id: unbound_pane.clone(),
                    binding_id: "unbound".into(),
                }
            )
            .contains("\"applied\":true"));
        assert!(app.state.runtime_binding_owners.contains_key("unbound"));

        app.handle_pane_close(
            "close-bound".into(),
            PaneTarget {
                pane_id: bound_pane,
            },
        );
        assert!(!app.state.runtime_binding_owners.contains_key("bound"));
        assert!(app.state.runtime_binding_owners.contains_key("unbound"));
        app.state.assert_invariants_for_test();

        app.handle_pane_close(
            "close-unbound".into(),
            PaneTarget {
                pane_id: unbound_pane,
            },
        );
        assert!(app.state.runtime_binding_owners.is_empty());
        app.state.assert_invariants_for_test();

        let rebound: SuccessResponse = serde_json::from_str(
            &app.handle_pane_bind_runtime("rebind".into(), bind(&survivor_pane, "bound")),
        )
        .unwrap();
        let ResponseResult::PaneRuntime {
            binding: Some(binding),
            ..
        } = rebound.result
        else {
            panic!("released binding should bind to a live pane")
        };
        assert_eq!(binding.pane_id, survivor_pane);
        assert_eq!(binding.seq, None);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_unbind_prunes_owners_of_missing_panes() {
        let mut app = app();
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        let bind = PaneBindRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
            attachment: attachment(),
        };
        app.handle_pane_bind_runtime("bind".into(), bind.clone());
        let mut stale = app.state.runtime_binding_owners["binding-a"].clone();
        stale.binding_id = "stale".into();
        stale.pane_id = format!("{}:p{}", app.state.workspaces[0].id, "ZZZ");
        app.state
            .runtime_binding_owners
            .insert("stale".into(), stale);
        assert!(app
            .handle_pane_unbind_runtime(
                "unbind".into(),
                PaneUnbindRuntimeParams {
                    pane_id: pane_id.clone(),
                    binding_id: "binding-a".into(),
                }
            )
            .contains("\"applied\":true"));
        assert_eq!(
            app.state.runtime_binding_owners.keys().collect::<Vec<_>>(),
            vec!["binding-a"]
        );
        assert_eq!(
            app.state.runtime_binding_owners["binding-a"].pane_id,
            pane_id
        );
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_observation_survives_tui_navigation_and_expires() {
        let mut app = app();
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        app.handle_pane_bind_runtime(
            "bind".into(),
            PaneBindRuntimeParams {
                pane_id: pane_id.clone(),
                binding_id: "binding-a".into(),
                attachment: attachment(),
            },
        );
        app.handle_pane_report_runtime(
            "report".into(),
            PaneReportRuntimeParams {
                pane_id: pane_id.clone(),
                binding_id: "binding-a".into(),
                seq: 1,
                state: RuntimeState::Done,
                label: None,
                ttl_ms: Some(1000),
            },
        );
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_hook_authority(
            "custom:tui".into(),
            "pi".into(),
            crate::detect::AgentState::Working,
            None,
            Some(1),
        );
        terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Blocked,
        );
        assert_eq!(
            terminal.runtime_agent_status(true),
            crate::api::schema::AgentStatus::Done
        );
        assert_eq!(
            terminal
                .runtime_binding
                .as_ref()
                .unwrap()
                .attachment
                .session_id,
            "ses_a"
        );
        let deadline = terminal.runtime_expiry().unwrap();
        terminal.expire_agent_metadata_at(deadline, deadline);
        assert_eq!(terminal.state, crate::detect::AgentState::Unknown);
        let current = terminal.runtime_binding_snapshot(deadline).unwrap();
        assert!(!current.fresh);
        assert_eq!(current.state, RuntimeState::Unknown);
        let saved = terminal.runtime_binding.clone();
        terminal.restore_runtime_binding(
            saved.as_ref(),
            crate::runtime_attachment::RuntimeBindingRestore::Relaunched,
            Instant::now(),
        );
        assert_eq!(terminal.runtime_binding.as_ref().unwrap().seq, Some(1));
        assert_eq!(
            terminal.runtime_agent_status(false),
            crate::api::schema::AgentStatus::Unknown
        );
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_rejects_credentials_and_changed_attach_identity() {
        let mut valid = attachment();
        assert!(valid.validate().is_ok());
        valid
            .attach_argv
            .extend(["--password".into(), "secret".into()]);
        assert!(valid.validate().is_err());
        let mut changed = attachment();
        changed.session_id = "ses_b".into();
        assert!(changed.validate().is_err());
    }

    fn with_prefix(prefix: &[String]) -> RuntimeAttachment {
        let mut attachment = attachment();
        attachment.attach_argv.splice(..1, prefix.iter().cloned());
        attachment
    }

    const PREFIX_REJECTED: &str = "restore requires an allowlisted shuvcode attach executable";

    #[test]
    fn runtime_attachment_accepts_allowlisted_attach_prefixes() {
        let root = std::env::temp_dir();
        let path = |name: &str| root.join(name).display().to_string();
        for prefix in [
            vec![path("shuvcode")],
            vec![path("shuvcode.exe")],
            vec![path("shuvcode.EXE")],
            vec![path("bun"), path("index.ts")],
            vec![path("bun.exe"), path("index.js")],
            vec![
                path("bun"),
                "--no-env-file".into(),
                "--preload".into(),
                path("preload.js"),
                path("index.ts"),
            ],
        ] {
            assert_eq!(with_prefix(&prefix).validate(), Ok(()), "{prefix:?}");
        }
    }

    #[test]
    fn runtime_attachment_rejects_non_allowlisted_attach_prefixes() {
        let root = std::env::temp_dir();
        let path = |name: &str| root.join(name).display().to_string();
        for prefix in [
            vec!["/usr/bin/env".to_string()],
            vec!["/bin/sh".to_string()],
            vec![path("shuvcode.sh")],
            vec![path("shuvcode-evil")],
            vec!["shuvcode".to_string()],
            vec![path("node"), path("index.ts")],
            vec![path("bun"), path("payload.ts")],
            vec![path("bun"), path("shuvcode")],
            vec![
                path("bun"),
                "--no-env-file".into(),
                "--preload".into(),
                path("payload.js"),
                path("index.ts"),
            ],
            vec![
                path("bun"),
                "--no-env-file".into(),
                "--preload".into(),
                path("preload.js"),
                path("payload.ts"),
            ],
        ] {
            assert_eq!(
                with_prefix(&prefix).validate(),
                Err(PREFIX_REJECTED),
                "{prefix:?}"
            );
        }
    }
    #[test]
    fn runtime_attachment_done_heartbeats_preserve_completion_sequence() {
        let mut app = app();
        app.state.outer_terminal_focus = Some(false);
        app.state.toast_config.delay_seconds = 1;
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        app.handle_pane_bind_runtime(
            "bind".into(),
            PaneBindRuntimeParams {
                pane_id: pane_id.clone(),
                binding_id: "binding-a".into(),
                attachment: attachment(),
            },
        );
        let report = |seq, state| PaneReportRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
            seq,
            state,
            label: None,
            ttl_ms: None,
        };
        app.handle_pane_report_runtime("idle".into(), report(1, RuntimeState::Idle));
        let (_, raw, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        let before = app.state.terminals[&terminal_id].last_agent_state_change_seq;
        app.handle_pane_report_runtime("done".into(), report(2, RuntimeState::Done));
        let completion = app.state.terminals[&terminal_id].last_agent_completion_seq;
        assert!(completion > before);
        let notification = app.state.pending_agent_notifications[&raw].clone();
        let seen = app.state.workspaces[0].pane_state(raw).unwrap().seen;
        app.handle_pane_report_runtime("heartbeat".into(), report(3, RuntimeState::Done));
        assert_eq!(
            app.state.terminals[&terminal_id].last_agent_completion_seq,
            completion
        );
        assert_eq!(
            app.state.terminals[&terminal_id].last_agent_state_change_seq,
            completion
        );
        assert_eq!(app.state.workspaces[0].pane_state(raw).unwrap().seen, seen);
        assert_eq!(
            app.state.pending_agent_notifications[&raw].deadline,
            notification.deadline
        );
        assert_eq!(
            app.pane_info(0, raw).unwrap().agent_status,
            crate::api::schema::AgentStatus::Done
        );
        assert!(!app.state.terminals[&terminal_id].runtime_presentation_seen(true));
        app.state.assert_invariants_for_test();
    }

    fn status_change_events(app: &App) -> usize {
        app.event_hub
            .events_after(0)
            .iter()
            .filter(|(_, envelope)| {
                matches!(
                    envelope.event,
                    crate::api::schema::EventKind::PaneAgentStatusChanged
                )
            })
            .count()
    }

    fn bound_unfocused_app() -> (App, String) {
        let mut app = app();
        app.state.outer_terminal_focus = Some(false);
        app.state.toast_config.delay_seconds = 1;
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        app.handle_pane_bind_runtime(
            "bind".into(),
            PaneBindRuntimeParams {
                pane_id: pane_id.clone(),
                binding_id: "binding-a".into(),
                attachment: attachment(),
            },
        );
        (app, pane_id)
    }

    fn report(pane_id: &str, seq: u64, state: RuntimeState) -> PaneReportRuntimeParams {
        PaneReportRuntimeParams {
            pane_id: pane_id.into(),
            binding_id: "binding-a".into(),
            seq,
            state,
            label: None,
            ttl_ms: None,
        }
    }

    // Reports a Done edge and asserts it produced exactly one completion.
    fn assert_single_done_completion(app: &mut App, pane_id: &str, seq: u64) {
        let (_, raw, terminal_id) = app.runtime_binding_target(pane_id).unwrap();
        app.state.pending_agent_notifications.clear();
        let change_seq = app.state.next_agent_state_change_seq;
        let events = status_change_events(app);
        assert!(app
            .handle_pane_report_runtime("done".into(), report(pane_id, seq, RuntimeState::Done))
            .contains("\"applied\":true"));
        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(app.state.next_agent_state_change_seq, change_seq + 1);
        assert_eq!(
            terminal.last_agent_completion_seq,
            Some(app.state.next_agent_state_change_seq)
        );
        assert_eq!(
            terminal.last_agent_state_change_seq,
            terminal.last_agent_completion_seq
        );
        assert_eq!(app.state.pending_agent_notifications.len(), 1);
        assert_eq!(
            app.state.pending_agent_notifications[&raw].kind,
            crate::app::state::ToastKind::Finished
        );
        assert!(!app.state.workspaces[0].pane_state(raw).unwrap().seen);
        assert_eq!(status_change_events(app), events + 1);
        assert_eq!(
            app.pane_info(0, raw).unwrap().agent_status,
            crate::api::schema::AgentStatus::Done
        );
    }

    #[test]
    fn runtime_attachment_unknown_to_done_publishes_completion() {
        let (mut app, pane_id) = bound_unfocused_app();
        let (_, raw, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        assert_eq!(
            app.state.terminals[&terminal_id].state,
            crate::detect::AgentState::Unknown
        );
        assert_single_done_completion(&mut app, &pane_id, 1);

        let completion = app.state.terminals[&terminal_id].last_agent_completion_seq;
        let deadline = app.state.pending_agent_notifications[&raw].deadline;
        let events = status_change_events(&app);
        app.handle_pane_report_runtime("heartbeat".into(), report(&pane_id, 2, RuntimeState::Done));
        assert_eq!(
            app.state.terminals[&terminal_id].last_agent_completion_seq,
            completion
        );
        assert_eq!(
            app.state.terminals[&terminal_id].last_agent_state_change_seq,
            completion
        );
        assert_eq!(
            app.state.pending_agent_notifications[&raw].deadline,
            deadline
        );
        assert_eq!(status_change_events(&app), events);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_idle_to_done_publishes_completion() {
        let (mut app, pane_id) = bound_unfocused_app();
        app.handle_pane_report_runtime("idle".into(), report(&pane_id, 1, RuntimeState::Idle));
        assert_single_done_completion(&mut app, &pane_id, 2);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_working_to_done_publishes_one_completion() {
        let (mut app, pane_id) = bound_unfocused_app();
        app.handle_pane_report_runtime(
            "working".into(),
            report(&pane_id, 1, RuntimeState::Working),
        );
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        assert_eq!(
            app.state.terminals[&terminal_id].state,
            crate::detect::AgentState::Working
        );
        assert_single_done_completion(&mut app, &pane_id, 2);
        app.state.assert_invariants_for_test();
    }

    fn expire_runtime_observation(app: &mut App, terminal_id: &TerminalId) {
        let now = Instant::now();
        let terminal = app.state.terminals.get_mut(terminal_id).unwrap();
        terminal.runtime_observed_at =
            Some(now - std::time::Duration::from_millis(crate::runtime_attachment::DEFAULT_TTL_MS));
        app.state.expire_agent_metadata_at(now, now);
        assert_eq!(
            app.state.terminals[terminal_id].state,
            crate::detect::AgentState::Unknown
        );
    }

    // Mirrors session capture followed by a cold restore that relaunches attach.
    fn cold_restore(app: &mut App, terminal_id: &TerminalId) {
        let now = Instant::now();
        let terminal = app.state.terminals.get_mut(terminal_id).unwrap();
        let saved = terminal.runtime_binding_snapshot(now);
        terminal.restore_runtime_binding(
            saved.as_ref(),
            crate::runtime_attachment::RuntimeBindingRestore::Relaunched,
            now,
        );
        assert_eq!(terminal.state, crate::detect::AgentState::Unknown);
    }

    #[test]
    fn runtime_attachment_expired_then_done_publishes_completion() {
        let (mut app, pane_id) = bound_unfocused_app();
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        app.handle_pane_report_runtime(
            "working".into(),
            report(&pane_id, 1, RuntimeState::Working),
        );
        expire_runtime_observation(&mut app, &terminal_id);
        assert_single_done_completion(&mut app, &pane_id, 2);

        // An expired Done is treated like any silent reporter: its next Done completes.
        expire_runtime_observation(&mut app, &terminal_id);
        assert_single_done_completion(&mut app, &pane_id, 3);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_restored_done_does_not_replay_notifications() {
        let (mut app, pane_id) = bound_unfocused_app();
        let (_, raw, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        assert_single_done_completion(&mut app, &pane_id, 1);
        cold_restore(&mut app, &terminal_id);
        app.state.pending_agent_notifications.clear();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .last_agent_completion_seq = None;

        for seq in [2, 3] {
            let change_seq = app.state.next_agent_state_change_seq;
            app.handle_pane_report_runtime(
                "done".into(),
                report(&pane_id, seq, RuntimeState::Done),
            );
            let terminal = &app.state.terminals[&terminal_id];
            assert!(app.state.pending_agent_notifications.is_empty());
            assert_eq!(terminal.last_agent_completion_seq, None);
            assert_eq!(terminal.state, crate::detect::AgentState::Idle);
            if seq == 2 {
                assert_eq!(app.state.next_agent_state_change_seq, change_seq + 1);
                assert_eq!(
                    terminal.last_agent_state_change_seq,
                    Some(app.state.next_agent_state_change_seq)
                );
            } else {
                assert_eq!(app.state.next_agent_state_change_seq, change_seq);
            }
            assert_eq!(
                app.pane_info(0, raw).unwrap().agent_status,
                crate::api::schema::AgentStatus::Done
            );
        }
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_restored_working_then_done_publishes_completion() {
        let (mut app, pane_id) = bound_unfocused_app();
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        app.handle_pane_report_runtime(
            "working".into(),
            report(&pane_id, 1, RuntimeState::Working),
        );
        cold_restore(&mut app, &terminal_id);
        assert_single_done_completion(&mut app, &pane_id, 2);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_done_stale_at_save_publishes_completion_after_restore() {
        let (mut app, pane_id) = bound_unfocused_app();
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        assert_single_done_completion(&mut app, &pane_id, 1);
        expire_runtime_observation(&mut app, &terminal_id);
        cold_restore(&mut app, &terminal_id);
        assert!(!app.state.terminals[&terminal_id].runtime_restored_done);
        assert_single_done_completion(&mut app, &pane_id, 2);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_moves_with_same_physical_pane() {
        let mut app = app();
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        app.handle_pane_bind_runtime(
            "bind".into(),
            PaneBindRuntimeParams {
                pane_id: pane_id.clone(),
                binding_id: "binding-a".into(),
                attachment: attachment(),
            },
        );
        let (_, _, terminal_id) = app.runtime_binding_target(&pane_id).unwrap();
        let moved: SuccessResponse = serde_json::from_str(&app.handle_pane_move(
            "move".into(),
            crate::api::schema::PaneMoveParams {
                pane_id: pane_id.clone(),
                destination: crate::api::schema::PaneMoveDestination::NewWorkspace {
                    label: None,
                    tab_label: None,
                },
                focus: false,
            },
        ))
        .unwrap();
        let ResponseResult::PaneMove { move_result } = moved.result else {
            panic!("move failed")
        };
        let new_id = move_result.pane.pane_id.clone();
        assert_ne!(pane_id, new_id);
        assert_eq!(app.runtime_binding_target(&new_id).unwrap().2, terminal_id);
        assert_eq!(
            app.state.runtime_binding_owners["binding-a"].pane_id,
            new_id
        );
        assert!(app
            .handle_pane_get_runtime("old".into(), PaneTarget { pane_id })
            .contains("pane_not_found"));
        assert!(app
            .handle_pane_report_runtime(
                "moved-report".into(),
                PaneReportRuntimeParams {
                    pane_id: new_id,
                    binding_id: "binding-a".into(),
                    seq: 1,
                    state: RuntimeState::Working,
                    label: None,
                    ttl_ms: None
                }
            )
            .contains("\"applied\":true"));
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn runtime_attachment_unbound_owner_follows_same_physical_pane_move() {
        let mut app = app();
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        let bind = PaneBindRuntimeParams {
            pane_id: pane_id.clone(),
            binding_id: "binding-a".into(),
            attachment: attachment(),
        };
        assert!(app
            .handle_pane_bind_runtime("bind".into(), bind.clone())
            .contains("\"applied\":true"));
        assert!(app
            .handle_pane_unbind_runtime(
                "unbind".into(),
                PaneUnbindRuntimeParams {
                    pane_id: pane_id.clone(),
                    binding_id: "binding-a".into(),
                }
            )
            .contains("\"applied\":true"));
        let terminal_id = app.runtime_binding_target(&pane_id).unwrap().2;
        let moved: SuccessResponse = serde_json::from_str(&app.handle_pane_move(
            "move".into(),
            crate::api::schema::PaneMoveParams {
                pane_id,
                destination: crate::api::schema::PaneMoveDestination::NewWorkspace {
                    label: None,
                    tab_label: None,
                },
                focus: false,
            },
        ))
        .unwrap();
        let ResponseResult::PaneMove { move_result } = moved.result else {
            panic!("move failed")
        };
        let mut moved_bind = bind;
        moved_bind.pane_id = move_result.pane.pane_id;
        assert_eq!(
            app.runtime_binding_target(&moved_bind.pane_id).unwrap().2,
            terminal_id
        );
        assert!(app
            .handle_pane_bind_runtime("rebind".into(), moved_bind.clone())
            .contains("\"applied\":true"));
        moved_bind.attachment.home_id = "changed-home".into();
        moved_bind.attachment.attach_argv[6] = "changed-home".into();
        assert!(app
            .handle_pane_bind_runtime("replace".into(), moved_bind)
            .contains("runtime_binding_conflict"));
        app.state.assert_invariants_for_test();
    }
}
