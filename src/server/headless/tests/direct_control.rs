use super::*;

fn with_controlled_pane(
    test: impl FnOnce(
        &mut HeadlessServer,
        crate::terminal::TerminalId,
        String,
        (u16, u16),
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::sync::mpsc::Receiver<Vec<u8>>,
        tokio::sync::mpsc::Receiver<Bytes>,
    ),
) {
    with_terminal_session_test_server(|server, terminal_id, target, pane_id| {
        let (runtime, input) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 16,
            );
        server
            .app
            .terminal_runtimes
            .insert(terminal_id.clone(), runtime);
        server.app.state.active = Some(0);
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.reconcile_client_shell_locations();
        assert!(server.claim_unowned_shell_tab_geometry(1, true));
        let size = server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .unwrap()
            .current_size();
        let observer = connect_pending_terminal_client_with_control_rx(server, 3);
        assert!(
            server.handle_server_event(ServerEvent::ClientObserveTerminal {
                client_id: 3,
                target: target.clone(),
            })
        );
        let controller = connect_pending_terminal_client_with_control_rx(server, 2);
        assert!(
            server.handle_server_event(ServerEvent::ClientControlTerminal {
                client_id: 2,
                target,
                takeover: true,
            })
        );
        assert_eq!(
            server
                .app
                .terminal_runtimes
                .get(&terminal_id)
                .unwrap()
                .current_size(),
            (30, 100)
        );
        test(
            server,
            terminal_id,
            pane_id,
            size,
            controller,
            observer,
            input,
        );
    });
}

#[test]
fn shell_activity_takes_over_direct_control_and_restores_geometry() {
    for trigger in [
        "focus",
        "repeat-focus",
        "pane-selection",
        "tab-selection",
        "workspace-selection",
        "input",
    ] {
        with_controlled_pane(
            |server, terminal_id, pane_id, size, controller, observer, mut input| {
                match trigger {
                    "focus" | "repeat-focus" => {
                        if trigger == "repeat-focus" {
                            server.clients.get_mut(&1).unwrap().outer_terminal_focus = Some(true);
                        }
                        assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
                            client_id: 1,
                            focused: true,
                        }));
                    }
                    "input" => {
                        assert!(
                            server.handle_server_event(ServerEvent::ClientShellPaneInput {
                                client_id: 1,
                                pane_id,
                                events: vec![protocol::ClientPaneInputEvent::TextCommit(
                                    "x".into()
                                )],
                            })
                        );
                        assert_eq!(input.try_recv().unwrap(), Bytes::from_static(b"x"));
                    }
                    _ => {
                        let method = match trigger {
                            "pane-selection" => {
                                api::schema::Method::PaneFocus(api::schema::PaneTarget { pane_id })
                            }
                            "tab-selection" => {
                                api::schema::Method::TabFocus(api::schema::TabTarget {
                                    tab_id: server.app.public_tab_id(0, 0).unwrap(),
                                })
                            }
                            _ => {
                                api::schema::Method::WorkspaceFocus(api::schema::WorkspaceTarget {
                                    workspace_id: server.app.public_workspace_id(0),
                                })
                            }
                        };
                        let (respond_to, response) = std::sync::mpsc::channel();
                        assert!(server.handle_client_shell_api_request(
                            1,
                            api::ApiRequestMessage {
                                request: api::schema::Request {
                                    id: trigger.into(),
                                    method
                                },
                                respond_to,
                                response_write_complete: None,
                                stream_active: None,
                            }
                        ));
                        assert!(serde_json::from_str::<api::schema::SuccessResponse>(
                            &response.try_recv().unwrap()
                        )
                        .is_ok());
                    }
                }
                assert_eq!(
                    read_server_shutdown_reason(controller.try_recv().unwrap()),
                    Some("terminal attach taken over".into())
                );
                assert!(!server.clients.contains_key(&2));
                assert!(!server
                    .terminal_attach_owners
                    .contains_key(terminal_id.as_str()));
                assert!(!server
                    .app
                    .state
                    .direct_attach_resize_locks
                    .contains(&terminal_id));
                assert_eq!(
                    server
                        .app
                        .terminal_runtimes
                        .get(&terminal_id)
                        .unwrap()
                        .current_size(),
                    size,
                    "{trigger}"
                );
                assert!(server.clients.contains_key(&3));
                assert!(observer.try_recv().is_err());

                // The phone can reacquire after a shell takeover, then another direct
                // controller can still take it over using the unchanged protocol.
                let phone = connect_pending_terminal_client_with_control_rx(server, 4);
                assert!(
                    server.handle_server_event(ServerEvent::ClientControlTerminal {
                        client_id: 4,
                        target: terminal_id.to_string(),
                        takeover: true,
                    })
                );
                let _next = connect_pending_terminal_client_with_control_rx(server, 5);
                assert!(
                    server.handle_server_event(ServerEvent::ClientControlTerminal {
                        client_id: 5,
                        target: terminal_id.to_string(),
                        takeover: true,
                    })
                );
                assert_eq!(
                    read_server_shutdown_reason(phone.try_recv().unwrap()),
                    Some("terminal attach taken over".into())
                );
                // A late disconnect from the old phone must not remove the new owner.
                server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 4 });
                assert_eq!(
                    server.terminal_attach_owners.get(terminal_id.as_str()),
                    Some(&5)
                );
                assert!(server
                    .app
                    .state
                    .direct_attach_resize_locks
                    .contains(&terminal_id));
                assert_eq!(
                    server
                        .app
                        .terminal_runtimes
                        .get(&terminal_id)
                        .unwrap()
                        .current_size(),
                    (30, 100)
                );
                assert!(server.clients.contains_key(&3));
                assert!(observer.try_recv().is_err());
            },
        );
    }
}

#[test]
fn passive_or_rejected_shell_activity_does_not_take_over_direct_control() {
    with_controlled_pane(
        |server, terminal_id, pane_id, _, controller, observer, _input| {
            server.handle_server_event(ServerEvent::ClientShellFocus {
                client_id: 1,
                focused: false,
            });
            server.handle_server_event(ServerEvent::ClientShellResize {
                client_id: 1,
                surface_cols: 150,
                surface_rows: 50,
                cell_width_px: 0,
                cell_height_px: 0,
                pixel_mouse: false,
            });
            server.handle_server_event(ServerEvent::ClientShellPaneInput {
                client_id: 1,
                pane_id: pane_id.clone(),
                events: vec![],
            });
            let (respond_to, response) = std::sync::mpsc::channel();
            server.handle_client_shell_api_request(
                1,
                api::ApiRequestMessage {
                    request: api::schema::Request {
                        id: "invalid".into(),
                        method: api::schema::Method::PaneFocus(api::schema::PaneTarget {
                            pane_id: "missing:p1".into(),
                        }),
                    },
                    respond_to,
                    response_write_complete: None,
                    stream_active: None,
                },
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(&response.try_recv().unwrap())
                    .unwrap()
                    .get("error")
                    .is_some()
            );
            // Observers cannot masquerade as a shell or change real terminal geometry.
            server.handle_server_event(ServerEvent::ClientShellFocus {
                client_id: 3,
                focused: true,
            });
            server.handle_server_event(ServerEvent::ClientShellPaneInput {
                client_id: 3,
                pane_id: pane_id.clone(),
                events: vec![protocol::ClientPaneInputEvent::TextCommit("ignored".into())],
            });
            server.handle_server_event(ServerEvent::ClientResize {
                client_id: 3,
                cols: 60,
                rows: 20,
                cell_width_px: 0,
                cell_height_px: 0,
                pixel_mouse: false,
            });
            server.clients.get_mut(&1).unwrap().shell_surface_active = false;
            server.handle_server_event(ServerEvent::ClientShellFocus {
                client_id: 1,
                focused: true,
            });
            server.handle_server_event(ServerEvent::ClientShellPaneInput {
                client_id: 1,
                pane_id,
                events: vec![protocol::ClientPaneInputEvent::TextCommit("ignored".into())],
            });
            assert_eq!(
                server.terminal_attach_owners.get(terminal_id.as_str()),
                Some(&2)
            );
            assert!(server
                .app
                .state
                .direct_attach_resize_locks
                .contains(&terminal_id));
            assert_eq!(
                server
                    .app
                    .terminal_runtimes
                    .get(&terminal_id)
                    .unwrap()
                    .current_size(),
                (30, 100)
            );
            assert!(controller.try_recv().is_err());
            assert!(observer.try_recv().is_err());
        },
    );
}

#[test]
fn shell_activity_in_another_tab_does_not_reclaim_hidden_controlled_pane() {
    with_controlled_pane(|server, terminal_id, pane_id, _, controller, _, _input| {
        let second_tab = server.app.state.workspaces[0].test_add_tab(Some("other"));
        let tab_id = server.app.public_tab_id(0, second_tab).unwrap();
        assert!(server.focus_shell_client_on_tab(1, &tab_id));
        server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id: 1,
            focused: true,
        });
        server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 1,
            pane_id,
            events: vec![protocol::ClientPaneInputEvent::TextCommit("stale".into())],
        });
        assert_eq!(
            server.terminal_attach_owners.get(terminal_id.as_str()),
            Some(&2)
        );
        assert!(server
            .app
            .state
            .direct_attach_resize_locks
            .contains(&terminal_id));
        assert!(controller.try_recv().is_err());
    });
}

#[test]
fn shell_image_paste_reclaims_direct_control() {
    with_controlled_pane(
        |server, terminal_id, pane_id, size, controller, observer, mut input| {
            assert!(server.paste_client_clipboard_image_path(
                1,
                protocol::ClientClipboardImageTarget::Pane(pane_id),
                "/tmp/test-image.png".into(),
            ));
            assert_eq!(
                read_server_shutdown_reason(controller.try_recv().unwrap()),
                Some("terminal attach taken over".into())
            );
            assert!(!server
                .app
                .state
                .direct_attach_resize_locks
                .contains(&terminal_id));
            assert_eq!(
                server
                    .app
                    .terminal_runtimes
                    .get(&terminal_id)
                    .unwrap()
                    .current_size(),
                size
            );
            assert_eq!(
                input.try_recv().unwrap(),
                Bytes::from_static(b"/tmp/test-image.png")
            );
            assert!(server.clients.contains_key(&3));
            assert!(observer.try_recv().is_err());
        },
    );
}
