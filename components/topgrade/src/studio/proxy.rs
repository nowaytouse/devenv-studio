use std::{
    env,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    process::Command,
    time::Duration,
};

const KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

pub(super) struct Route(bool);

impl Route {
    pub(super) fn detect() -> Self {
        for key in KEYS {
            let Ok(value) = env::var(key) else { continue };
            let Some((host, port)) = loopback(&value) else { continue };
            let timeout = Duration::from_secs(1);
            if TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout).is_err()
                && (host != "localhost"
                    || TcpStream::connect_timeout(&SocketAddr::from((Ipv6Addr::LOCALHOST, port)), timeout).is_err())
            {
                eprintln!("Loopback proxy {host}:{port} is offline; using direct/VIF");
                return Self(true);
            }
        }
        Self(false)
    }

    pub(super) fn apply(&self, command: &mut Command) {
        if self.0 {
            for key in KEYS {
                command.env_remove(key);
            }
            command.env("PROXY_AUTO_FALLBACK", "direct");
        }
    }
}

fn loopback(value: &str) -> Option<(&str, u16)> {
    let authority = value.split_once("://").map_or(value, |(_, rest)| rest);
    let (host, port) = authority.split('/').next()?.split_once(':')?;
    matches!(host, "127.0.0.1" | "localhost").then_some((host, port.parse().ok()?))
}
