// The first seven tests mirror the Hub's CONNECT destination pattern tests. Keep
// the shared cases in sync; the routing-policy tests below cover Wizard's
// intentional direct-route behavior.
use super::{
    ALLOWED_DESTINATIONS, ConnectDestination, ConnectDestinationPattern, Route, route,
    route_destination,
};
use std::collections::HashSet;

#[test]
fn exact_patterns_preserve_concrete_destination_grammar() {
    for value in [
        "localhost:80",
        "API2.Cursor.SH:00443",
        "a-b.example.com:65535",
        "xn--bcher-kva.example:443",
    ] {
        let destination = value
            .parse::<ConnectDestination>()
            .expect("valid destination");
        let pattern = value
            .parse::<ConnectDestinationPattern>()
            .expect("valid exact pattern");
        assert_eq!(pattern.host(), destination.host());
        assert_eq!(pattern.to_string(), destination.to_string());
        assert!(pattern.matches(&destination));
        assert_eq!(pattern, ConnectDestinationPattern::from(destination));
    }

    let pattern: ConnectDestinationPattern = "api2.cursor.sh:443".parse().expect("valid pattern");
    for value in [
        "cursor.sh:443",
        "child.api2.cursor.sh:443",
        "api2.cursor.sh:8443",
    ] {
        assert!(
            !pattern.matches(&value.parse().expect("valid destination")),
            "{value}"
        );
    }
}

#[test]
fn single_star_matches_exactly_one_label() {
    let pattern: ConnectDestinationPattern = "*.CursorVM.com:00443".parse().expect("valid pattern");
    assert_eq!(pattern.to_string(), "*.cursorvm.com:443");

    for value in [
        "cluster.cursorvm.com:443",
        "A-B.CursorVM.com:443",
        "0.cursorvm.com:443",
    ] {
        let destination = value.parse().expect("valid destination");
        assert!(pattern.matches(&destination), "{value}");
    }
    for value in [
        "cursorvm.com:443",
        "computer.cluster.cursorvm.com:443",
        "cluster.cursorvm.com:8443",
        "cluster.evilcursorvm.com:443",
        "cluster.cursorvm.com.evil:443",
        "cluster.cursorvm-com:443",
        "cursorvm.com.evil:443",
    ] {
        let destination = value.parse().expect("valid destination");
        assert!(!pattern.matches(&destination), "{value}");
    }
}

#[test]
fn double_star_matches_apex_and_any_depth() {
    let pattern: ConnectDestinationPattern = "**.OpenAI.com:00443".parse().expect("valid pattern");
    assert_eq!(pattern.to_string(), "**.openai.com:443");
    assert_eq!(pattern.host(), "**.openai.com");

    for value in [
        "openai.com:443",
        "b.openai.com:443",
        "A.B.OpenAI.com:443",
        "a.b.c.d.openai.com:443",
    ] {
        let destination = value.parse().expect("valid destination");
        assert!(pattern.matches(&destination), "{value}");
    }
    for value in [
        "evilopenai.com:443",
        "a.evilopenai.com:443",
        "openai.com.evil:443",
        "a.openai.com.evil:443",
        "openai-com:443",
        "a.openai-com:443",
        "openai.co:443",
        "openai.com:8443",
        "a.b.openai.com:80",
    ] {
        let destination = value.parse().expect("valid destination");
        assert!(!pattern.matches(&destination), "{value}");
    }
}

#[test]
fn wildcard_patterns_reject_unbounded_and_partial_wildcards() {
    for value in [
        "*:443",
        "**:443",
        "*.*:443",
        "*.com:443",
        "**.com:443",
        "*.*.com:443",
        "*.localhost:443",
        "**.localhost:443",
        "*.*.cursorvm.com:443",
        "**.**.cursorvm.com:443",
        "*.**.cursorvm.com:443",
        "**.*.cursorvm.com:443",
        "***.cursorvm.com:443",
        "**cursorvm.com:443",
        "**.cursorvm.com.**:443",
        "foo*.cursorvm.com:443",
        "*foo.cursorvm.com:443",
        "**foo.cursorvm.com:443",
        "foo.*.cursorvm.com:443",
        "foo.**.cursorvm.com:443",
        "*.foo.*.cursorvm.com:443",
        "*.cursorvm.*:443",
        "**.cursorvm.*:443",
        "api2.cursor.sh:*",
        "**.openai.com:*",
    ] {
        assert!(
            value.parse::<ConnectDestinationPattern>().is_err(),
            "{value}"
        );
    }
}

#[test]
fn patterns_reject_invalid_hosts_and_ports() {
    for value in [
        "*.cursorvm.com",
        "*.cursorvm.com:",
        "*.cursorvm.com:0",
        "*.cursorvm.com:65536",
        "*.cursorvm.com:-443",
        "*.cursorvm.com:+443",
        "*.cursorvm.com:4_43",
        "*.cursorvm.com:４４３",
        " *.cursorvm.com:443",
        "*.cursorvm.com:443 ",
        "*.cursorvm.com.:443",
        "*..cursorvm.com:443",
        "*.foo..com:443",
        "*.-foo.com:443",
        "*.foo-.com:443",
        "*.foo_bar.com:443",
        "*.cürsorvm.com:443",
        "*.127.0.0.1:443",
        "**.127.0.0.1:443",
        "*.127.1:443",
        "*.0x7f.1:443",
        "*.[::1]:443",
        "https://*.cursorvm.com:443",
        "*.cursorvm.com:443/path",
        "*.cursorvm.com:443?query",
        "user@*.cursorvm.com:443",
        "%2a.cursorvm.com:443",
        "127.0.0.1:443",
        "[::1]:443",
        "cursorvm.com:0",
        "cursorvm.com:65536",
        "cursorvm.com.:443",
    ] {
        assert!(
            value.parse::<ConnectDestinationPattern>().is_err(),
            "{value}"
        );
    }
}

#[test]
fn patterns_enforce_dns_label_and_total_length_limits() {
    let label = "a".repeat(63);
    let max_host = format!("*.{label}.{label}.{label}.{}", "b".repeat(59));
    assert_eq!(max_host.len(), 253);
    let pattern = format!("{max_host}:443")
        .parse::<ConnectDestinationPattern>()
        .expect("253-byte host pattern is valid");
    let destination = format!("x{}:443", &max_host[1..])
        .parse()
        .expect("valid destination");
    assert!(pattern.matches(&destination));

    let any_depth = format!("*{max_host}:443")
        .parse::<ConnectDestinationPattern>()
        .expect("`**.` prefix is not counted toward 253 bytes");
    let apex = format!("{}:443", &max_host[2..])
        .parse()
        .expect("valid destination");
    assert!(any_depth.matches(&apex));
    assert!(any_depth.matches(&destination));

    for value in [
        format!("{max_host}c:443"),
        format!("*.{}.com:443", "a".repeat(64)),
    ] {
        assert!(
            value.parse::<ConnectDestinationPattern>().is_err(),
            "{value}"
        );
    }
}

#[test]
fn concrete_destinations_still_reject_wildcards() {
    for value in [
        "*.cursorvm.com:443",
        "**.cursorvm.com:443",
        "foo*.cursorvm.com:443",
        "*:443",
    ] {
        assert!(value.parse::<ConnectDestination>().is_err(), "{value}");
    }
}

#[test]
fn built_in_list_is_valid_unique_and_routes_representative_destinations() {
    let mut seen = HashSet::new();
    for value in ALLOWED_DESTINATIONS {
        let pattern = value
            .parse::<ConnectDestinationPattern>()
            .expect("valid built-in pattern");
        assert!(seen.insert(pattern), "duplicate destination");
        // Exercise every configured pattern through the actual routing entry point.
        let destination = value.replace("**", "a.b").replace('*', "test");
        assert_eq!(route(&destination), Ok(Route::Proxy), "{value}");
        for port in [1, 80, 8443, 65535] {
            assert_eq!(
                route(&destination.replace(":443", &format!(":{port}"))),
                Ok(Route::Block),
                "{value} on {port}"
            );
        }
    }
    assert_eq!(route("API.OpenAI.COM:00443"), Ok(Route::Proxy));
    assert_eq!(route("PROD.Authentication.Cursor.SH:443"), Ok(Route::Proxy));
    for value in [
        "openai.com:443",
        "sub.api.openai.com:443",
        "chatgpt.com:443",
        "persistent.oaistatic.com:443",
        "claude.com:443",
        "claude.ai:443",
        "anthropic.com:443",
        "sub.api.anthropic.com:443",
        "bridge.claudeusercontent.com:443",
        "cursorvm.com:443",
        "a.b.c.cursorvm.com:443",
    ] {
        assert_eq!(route(value), Ok(Route::Proxy), "{value}");
    }
    for value in [
        "sub.api.openai.com:80",
        "downloads.claude.ai:80",
        "cursorvm.com:80",
        "a.b.c.cursorvm.com:8443",
    ] {
        assert_eq!(route(value), Ok(Route::Block), "{value}");
    }
}

#[test]
fn full_matches_take_precedence_over_host_only_matches() {
    let patterns = ["*.example.com:443", "api.example.com:8443"]
        .map(|value| value.parse().expect("valid pattern"));
    for (value, expected) in [
        ("API.Example.COM:00443", Route::Proxy),
        ("API.Example.COM:08443", Route::Proxy),
        ("api.example.com:80", Route::Block),
        ("other.example.com:8443", Route::Block),
        ("example.com:80", Route::Direct),
    ] {
        assert_eq!(
            route_destination(value.parse().expect("valid destination"), &patterns),
            expected,
            "{value}"
        );
    }
}

#[test]
fn only_valid_unmatched_destinations_go_direct() {
    for value in [
        "example.com:443",
        "example.com:80",
        "api.openai.com.example.com:8443",
        "api.openai.com.example.com:443",
        "evilopenai.com:443",
        "a.evilcursorvm.com:8443",
        "cursor.sh:443",
        "a.b.cursor.sh:443",
        "cursorapi.com:80",
        "127.0.0.1:443",
        "[::1]:443",
        "[2001:db8::1]:8080",
    ] {
        assert_eq!(route(value), Ok(Route::Direct), "{value}");
    }
    for value in [
        "api.openai.com.:443",
        "api..openai.com:443",
        "api_openai.com:443",
        "-api.openai.com:443",
        "cürsorvm.com:443",
        "api.openai.com",
        "api.openai.com:",
        "api.openai.com:+443",
        "api.openai.com:0",
        "api.openai.com:65536",
        "127.0.0.1:0",
        "[::1]:0",
        "127.1:443",
        "0x7f.1:443",
        "2130706433:443",
        "*.cursor.sh:443",
        "user@api.openai.com:443",
        " api.openai.com:443",
    ] {
        assert!(route(value).is_err(), "{value}");
    }
}
