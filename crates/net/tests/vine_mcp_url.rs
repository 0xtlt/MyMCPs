//! Port of the "validators: MCP endpoint URL" group of
//! `tests/unit/vine_mcp_url.spec.ts`, for the part `parseHttpUrl` decides.
//! The validator that calls it, and the flashed values, are tested with it.

use mymcps_net::parse_http_url;

#[test]
fn accepts_an_http_s_endpoint() {
    for http_url in [
        "https://mcp.example.com/mcp",
        "http://127.0.0.1:9999/mcp",
        "https://mcp.example.com/mcp?key=value",
        "https://user:secret@mcp.example.com/mcp",
    ] {
        let url = parse_http_url(http_url, "MCP URL").unwrap();
        assert_eq!(url.as_str(), http_url);
    }
}

#[test]
fn refuses_an_endpoint_that_is_not_plain_http_s() {
    for (http_url, message) in [
        (
            "ftp://mcp.example.com/mcp",
            "MCP URL must use HTTP or HTTPS",
        ),
        ("mcp.example.com/mcp", "MCP URL must be a valid URL"),
        (
            "https://mcp.example.com/mcp#tools",
            "MCP URL must not include a fragment",
        ),
    ] {
        let error = parse_http_url(http_url, "MCP URL").unwrap_err();
        assert_eq!(error.to_string(), message, "{http_url}");
    }
}
