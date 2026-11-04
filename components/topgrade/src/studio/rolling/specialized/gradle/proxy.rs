use std::{env, net::IpAddr, path::Path, process::Command};

use color_eyre::eyre::{ContextCompat, Result, ensure};
use regex::Regex;

use super::files;

fn value(command: &Command, key: &str) -> Option<String> {
    match command.get_envs().find(|(name, _)| *name == key) {
        Some((_, value)) => value.and_then(|value| value.to_str()).map(ToOwned::to_owned),
        None => env::var(key).ok(),
    }
}

fn proxy_value(command: &Command, key: &str) -> String {
    value(command, key)
        .or_else(|| value(command, &key.to_ascii_uppercase()))
        .unwrap_or_default()
}

fn bare_host(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn parse_proxy(proxy: &str) -> Result<(&str, u16)> {
    let authority = proxy
        .strip_prefix("http://")
        .context("Gradle proxy inheritance requires an HTTP proxy")?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    ensure!(
        !authority.contains(['@', '/', '?', '#']),
        "Gradle proxy inheritance requires an unauthenticated HTTP proxy"
    );
    let (host, port) = if let Some(ip) = authority.strip_prefix('[') {
        let (address, port) = ip.split_once(']').context("Invalid Gradle proxy address")?;
        ensure!(
            address.parse::<std::net::Ipv6Addr>().is_ok(),
            "Invalid Gradle proxy IPv6 address"
        );
        let port = if port.is_empty() {
            None
        } else {
            Some(port.strip_prefix(':').context("Invalid Gradle proxy port")?)
        };
        (address, port)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        ensure!(bare_host(host), "Invalid Gradle proxy host");
        (host, port)
    };
    let port: u16 = port.map(str::parse).transpose()?.unwrap_or(80);
    ensure!(port != 0, "Invalid Gradle proxy port");
    Ok((host, port))
}

fn non_proxy_hosts(value: &str) -> Result<Vec<String>> {
    let mut patterns = Vec::new();
    for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
        let ip = token.trim_matches(['[', ']']).parse::<IpAddr>();
        if let Ok(ip) = ip {
            patterns.push(if ip.is_ipv6() {
                format!("[{ip}]")
            } else {
                ip.to_string()
            });
        } else {
            let host = token.strip_prefix('.').unwrap_or(token);
            ensure!(
                bare_host(host),
                "NO_PROXY cannot be mapped to Gradle; configure JVM nonProxyHosts explicitly"
            );
            patterns.push(host.to_owned());
            patterns.push(format!("*.{host}"));
        }
    }
    patterns.dedup();
    Ok(patterns)
}

pub(super) fn gradle_args(command: &Command, root: &Path, home: &Path) -> Result<Vec<String>> {
    let explicit = Regex::new(r"-D(?:https?\.(?:proxy|nonProxy)|socksProxy|java\.net\.useSystemProxies)")?;
    if ["JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS", "JAVA_OPTS", "GRADLE_OPTS"]
        .into_iter()
        .any(|key| value(command, key).is_some_and(|value| explicit.is_match(&value)))
    {
        return Ok(Vec::new());
    }
    let home = value(command, "GRADLE_USER_HOME").map_or_else(|| home.join(".gradle"), |path| root.join(path));
    let system_property =
        Regex::new(r"(?m)^\s*systemProp\.(?:https?\.(?:proxy|nonProxy)|socksProxy|java\.net\.useSystemProxies)")?;
    let continuation = Regex::new(r"\\\r?\n[ \t]*")?;
    for path in [home.join("gradle.properties"), root.join("gradle.properties")] {
        if super::super::super::super::exists(&path)? {
            let property = files::read(&path, 1024 * 1024)?;
            let text = std::str::from_utf8(&property.bytes)?;
            if system_property.is_match(text)
                || continuation
                    .replace_all(text, "")
                    .lines()
                    .any(|line| line.trim_start().starts_with("org.gradle.jvmargs") && explicit.is_match(line))
            {
                return Ok(Vec::new());
            }
        }
    }
    let proxies = [
        ("http", proxy_value(command, "http_proxy")),
        ("https", proxy_value(command, "https_proxy")),
    ];
    if proxies.iter().all(|(_, proxy)| proxy.is_empty()) {
        ensure!(
            proxy_value(command, "all_proxy").is_empty(),
            "Gradle needs explicit JVM settings for an ALL_PROXY-only route"
        );
        return Ok(Vec::new());
    }
    let bypass = proxy_value(command, "no_proxy");
    if bypass.trim() == "*" {
        return Ok(Vec::new());
    }
    let mut args = Vec::new();
    for (scheme, proxy) in &proxies {
        if !proxy.is_empty() {
            let (host, port) = parse_proxy(proxy)?;
            args.extend([
                format!("-D{scheme}.proxyHost={host}"),
                format!("-D{scheme}.proxyPort={port}"),
            ]);
        }
    }
    let patterns = non_proxy_hosts(&bypass)?;
    if !patterns.is_empty() {
        args.push(format!("-Dhttp.nonProxyHosts={}", patterns.join("|")));
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_or_ambiguous_routes_require_explicit_jvm_configuration() -> Result<()> {
        assert_eq!(parse_proxy("http://[::1]:7890")?, ("::1", 7890));
        assert!(parse_proxy("http://secret@127.0.0.1:7890").is_err());
        assert!(parse_proxy("socks5://127.0.0.1:7890").is_err());
        assert!(non_proxy_hosts("127.0.0.0/8").is_err());
        assert_eq!(non_proxy_hosts("localhost,::1")?, ["localhost", "*.localhost", "[::1]"]);
        Ok(())
    }
}
