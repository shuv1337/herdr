use std::path::Path;
use std::time::{Duration, Instant};

use crate::api::schema::{RuntimeAttachment, RuntimeBinding, RuntimeState};
use crate::detect::AgentState;
use crate::terminal::{TerminalState, TerminalStateMutation};

pub(crate) const METHODS: &[&str] = &[
    "pane.bind_runtime",
    "pane.get_runtime",
    "pane.report_runtime",
    "pane.unbind_runtime",
];
pub(crate) const DEFAULT_TTL_MS: u64 = 30_000;

impl RuntimeAttachment {
    // Only the read-only native attachment entrypoint may be persisted for restore.
    // The local command API already grants terminal input authority. This validates
    // the trusted caller's attach descriptor shape, not executable/script contents.
    // Source and compiled commands remain caller-owned; credential flags are excluded.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.provider != "shuvcode"
            || [
                self.home_id.as_str(),
                self.session_id.as_str(),
                self.host_id.as_str(),
            ]
            .iter()
            .any(|value| {
                value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
            })
            || !Path::new(&self.location).is_absolute()
            || self
                .attach_argv
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.chars().any(char::is_control))
        {
            return Err("invalid runtime attachment identity");
        }
        let Some(at) = self
            .attach_argv
            .windows(2)
            .position(|args| args == ["supervisor", "attach"])
        else {
            return Err("restore requires exact supervisor attach argv");
        };
        let prefix = &self.attach_argv[..at];
        let absolute = |value: &String| Path::new(value).is_absolute();
        let valid_prefix = match prefix {
            [executable] => absolute(executable),
            [bun, entrypoint] => absolute(bun) && absolute(entrypoint),
            [bun, no_env, preload_flag, preload, entrypoint] => {
                absolute(bun)
                    && no_env == "--no-env-file"
                    && preload_flag == "--preload"
                    && absolute(preload)
                    && absolute(entrypoint)
            }
            _ => false,
        };
        let suffix = &self.attach_argv[at + 2..];
        if !valid_prefix
            || suffix.len() != 8
            || suffix[0] != "--home"
            || !Path::new(&suffix[1]).is_absolute()
            || suffix[2] != "--home-id"
            || suffix[3] != self.home_id
            || suffix[4] != "--session"
            || suffix[5] != self.session_id
            || suffix[6] != "--location"
            || suffix[7] != self.location
        {
            return Err("restore requires exact credential-free native attach argv");
        }
        Ok(())
    }
}

impl RuntimeState {
    pub(crate) fn detect(self) -> AgentState {
        match self {
            Self::Idle | Self::Done => AgentState::Idle,
            Self::Working => AgentState::Working,
            Self::Blocked => AgentState::Blocked,
            Self::Unknown => AgentState::Unknown,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }
}

impl TerminalState {
    pub(crate) fn runtime_agent_status(&self, seen: bool) -> crate::api::schema::AgentStatus {
        if let Some(binding) = &self.runtime_binding {
            if self
                .runtime_expiry()
                .is_none_or(|deadline| Instant::now() >= deadline)
            {
                return crate::api::schema::AgentStatus::Unknown;
            }
            return match binding.state {
                RuntimeState::Idle => crate::api::schema::AgentStatus::Idle,
                RuntimeState::Working => crate::api::schema::AgentStatus::Working,
                RuntimeState::Blocked => crate::api::schema::AgentStatus::Blocked,
                RuntimeState::Done => crate::api::schema::AgentStatus::Done,
                RuntimeState::Unknown => crate::api::schema::AgentStatus::Unknown,
            };
        }
        crate::app::api_helpers::pane_agent_status(self.state, seen)
    }

    // Scalar-only projection for existing internal attention/status paths.
    // The pane's actual acknowledgement remains independent of managed state.
    pub(crate) fn runtime_presentation_seen(&self, seen: bool) -> bool {
        let Some(binding) = &self.runtime_binding else {
            return seen;
        };
        if self
            .runtime_expiry()
            .is_none_or(|deadline| Instant::now() >= deadline)
        {
            return seen;
        }
        match binding.state {
            RuntimeState::Idle => true,
            RuntimeState::Done => false,
            _ => seen,
        }
    }

    pub(crate) fn runtime_expiry(&self) -> Option<Instant> {
        Some(
            self.runtime_observed_at?
                + Duration::from_millis(self.runtime_binding.as_ref()?.ttl_ms),
        )
    }

    pub(crate) fn runtime_state_at(&self, now: Instant) -> AgentState {
        if let Some(binding) = &self.runtime_binding {
            if self.runtime_expiry().is_some_and(|deadline| now < deadline) {
                return binding.state.detect();
            }
        }
        AgentState::Unknown
    }

    pub(crate) fn runtime_binding_snapshot(&self, now: Instant) -> Option<RuntimeBinding> {
        let mut binding = self.runtime_binding.clone()?;
        binding.fresh = self.runtime_expiry().is_some_and(|deadline| now < deadline);
        if !binding.fresh {
            binding.state = RuntimeState::Unknown;
            binding.label = None;
        }
        Some(binding)
    }

    pub(crate) fn mutate_runtime_binding(
        &mut self,
        binding: Option<RuntimeBinding>,
        observed_at: Option<Instant>,
    ) -> TerminalStateMutation {
        let previous = self.unchanged_effective_state_change_at(Instant::now());
        let identity_changed = self
            .runtime_binding
            .as_ref()
            .map(|binding| (&binding.binding_id, &binding.attachment))
            != binding
                .as_ref()
                .map(|binding| (&binding.binding_id, &binding.attachment));
        if binding.is_none() && self.runtime_binding.is_some() {
            self.pending_agent_resume_plan = None;
        }
        self.runtime_binding = binding;
        self.runtime_observed_at = observed_at;
        TerminalStateMutation {
            effective_state_change: self.recompute_effective_state(
                previous.agent_label,
                previous.known_agent,
                previous.state,
                previous.presentation,
                Instant::now(),
            ),
            session_ref_changed: identity_changed,
            agent_released: false,
        }
    }

    pub(crate) fn restore_runtime_binding(&mut self, binding: Option<&RuntimeBinding>) {
        self.runtime_binding = binding.cloned().map(|mut binding| {
            binding.state = RuntimeState::Unknown;
            binding.label = None;
            binding.fresh = false;
            binding
        });
        self.runtime_observed_at = None;
        if self.runtime_binding.is_some() {
            self.state = AgentState::Unknown;
        }
    }
}
