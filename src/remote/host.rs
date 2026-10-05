//! Remote-host side of the SSH stdio bridge.

use std::io;
use std::time::Duration;

pub(crate) fn run_remote_client_bridge(args: &[String]) -> io::Result<()> {
    let idle_timeout = match args {
        [] => false,
        [option]
            if option == "--idle-timeout-v1"
                && crate::platform::REMOTE_BRIDGE_IDLE_TIMEOUT_SUPPORTED =>
        {
            true
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported remote client bridge option",
            ))
        }
    };
    ensure_remote_server_running()?;
    #[cfg(unix)]
    let _ssh_agent = super::ssh_agent::Registration::start();

    let socket_path = crate::server::socket_paths::client_socket_path();
    let stream = crate::ipc::connect_local_stream(&socket_path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to connect to remote Herdr client socket {}: {err}",
                socket_path.display()
            ),
        )
    })?;

    crate::platform::forward_remote_bridge_stdio(stream, idle_timeout)
}

fn ensure_remote_server_running() -> io::Result<()> {
    let socket_path = crate::server::socket_paths::client_socket_path();
    if crate::server::autodetect::is_server_listening() {
        let status = crate::api::read_runtime_status_at(
            &crate::api::socket_path(),
            Duration::from_millis(500),
        )?
        .ok_or_else(|| io::Error::other("remote server status API is unavailable"))?;
        if status
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.endpoint_protocol_generation)
            == Some(crate::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION)
        {
            return Ok(());
        }
        return Err(io::Error::other(
            "remote herdr server needs one final update before this bridge can attach; rerun `herdr --remote` from an interactive terminal to approve it",
        ));
    }

    start_remote_server(
        &socket_path,
        crate::server::autodetect::spawn_and_wait_for_server,
    )
}

// Keep existing-server validation unchanged. Only the absent-server launch is
// injected, so local and remote owned startup share the same exit diagnostics.
fn start_remote_server(
    socket_path: &std::path::Path,
    start: impl FnOnce(&std::path::Path, Duration) -> io::Result<()>,
) -> io::Result<()> {
    start(socket_path, Duration::from_secs(5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_owned_startup_propagates_exit_and_uses_five_second_deadline() {
        let error = start_remote_server(
            std::path::Path::new("/remote/client.sock"),
            |socket, timeout| {
                assert_eq!(timeout, Duration::from_secs(5));
                crate::server::autodetect::wait_for_startup(
                    socket,
                    std::path::Path::new("/remote/session/herdr-server.log"),
                    timeout,
                    || Ok(false),
                    || Ok(Some("exit status: 23".to_owned())),
                    || Duration::ZERO,
                    |_| panic!("dead startup must not wait"),
                )
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("exit status: 23"));
        assert!(error
            .to_string()
            .contains("/remote/session/herdr-server.log"));
    }

    #[test]
    fn remote_owned_startup_allows_slow_bind_and_reports_healthy_timeout() {
        for bind_at in [Some(Duration::from_secs(4)), None] {
            let elapsed = std::cell::Cell::new(Duration::ZERO);
            let result = start_remote_server(
                std::path::Path::new("/remote/client.sock"),
                |socket, timeout| {
                    crate::server::autodetect::wait_for_startup(
                        socket,
                        std::path::Path::new("/remote/session/herdr-server.log"),
                        timeout,
                        || Ok(bind_at.is_some_and(|bind| elapsed.get() >= bind)),
                        || Ok(None),
                        || elapsed.get(),
                        |delay| elapsed.set(elapsed.get() + delay),
                    )
                },
            );
            if let Some(bind_at) = bind_at {
                assert!(result.is_ok());
                assert_eq!(elapsed.get(), bind_at);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert!(error.to_string().contains("within 5s"));
                assert!(error.to_string().contains("may still be starting"));
                assert_eq!(elapsed.get(), Duration::from_secs(5));
            }
        }
    }
}
