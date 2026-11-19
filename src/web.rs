use anyhow::{Context, Result, bail, ensure};
use reqwest::{Url, blocking::Client, redirect::Policy};
use scraper::{ElementRef, Html, Node, Selector};
use serde_json::Value;
use std::{
    io::Read,
    net::{IpAddr, ToSocketAddrs},
    time::Duration,
};

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_documentation()
                || a == 0
                || a >= 224
                || a == 100 && (64..128).contains(&b)
                || a == 198 && (b == 18 || b == 19)
                || a == 192 && b == 0 && c == 0
                || a == 192 && b == 88 && c == 99)
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && s[0] != 0x2002
                && (s[0] & 0xfff0) != 0x3ff0
        }
    }
}
fn fetch(mut url: Url) -> Result<(String, String)> {
    for _ in 0..6 {
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port_or_known_default() == Some(443),
            "Only public HTTPS URLs on port 443 are supported"
        );
        let host = url
            .host_str()
            .context("Missing hostname")?
            .trim_matches(['[', ']']);
        let addrs: Vec<_> = (host, 443).to_socket_addrs()?.collect();
        ensure!(
            !addrs.is_empty() && addrs.iter().all(|a| public(a.ip())),
            "Destination is not a public address"
        );
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .resolve_to_addrs(host, &addrs)
            .user_agent("Luma/0.2 (opt-in research)")
            .build()?;
        let response = client.get(url.clone()).send()?;
        if response.status().is_redirection() {
            url = url.join(
                response
                    .headers()
                    .get("location")
                    .context("Missing redirect location")?
                    .to_str()?,
            )?;
            continue;
        }
        let response = response.error_for_status()?;
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        ensure!(
            matches!(
                kind.as_str(),
                "text/html" | "text/plain" | "application/xhtml+xml"
            ),
            "Unsupported web content type"
        );
        let mut data = Vec::new();
        response.take(2_000_001).read_to_end(&mut data)?;
        ensure!(data.len() <= 2_000_000, "Page exceeds 2 MB");
        return Ok((String::from_utf8_lossy(&data).into_owned(), kind));
    }
    bail!("Too many redirects")
}
pub fn run(name: &str, args: &Value) -> Result<String> {
    if name == "web_search" {
        let q = crate::workspace::string(args, "query")?;
        ensure!(
            !q.trim().is_empty() && q.chars().count() <= 500,
            "Invalid query"
        );
        println!("Searching DuckDuckGo for {}", crate::safe(q));
        let mut url = Url::parse("https://html.duckduckgo.com/html/")?;
        url.query_pairs_mut().append_pair("q", q);
        let (body, _) = fetch(url)?;
        let page = Html::parse_document(&body);
        let rows = Selector::parse(".result").unwrap();
        let links = Selector::parse("a.result__a").unwrap();
        let snippets = Selector::parse(".result__snippet").unwrap();
        let mut hits = Vec::new();
        for row in page.select(&rows).take(5) {
            if let Some(a) = row.select(&links).next() {
                let raw = a.value().attr("href").unwrap_or("");
                let href = Url::parse("https://duckduckgo.com/")?.join(raw)?;
                let dest = href
                    .query_pairs()
                    .find(|(k, _)| k == "uddg")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or(href.into());
                hits.push(format!(
                    "{}\n{}\n{}",
                    a.text().collect::<String>(),
                    dest,
                    row.select(&snippets)
                        .next()
                        .map(|s| s.text().collect::<String>())
                        .unwrap_or_default()
                ));
            }
        }
        return Ok(if hits.is_empty() {
            "No parseable search results.".into()
        } else {
            hits.join("\n\n")
        });
    }
    let url = crate::workspace::string(args, "url")?;
    ensure!(url.len() <= 2048, "URL too long");
    println!("Fetching {}", crate::safe(url));
    let (body, kind) = fetch(Url::parse(url)?)?;
    let text = if kind == "text/plain" {
        body
    } else {
        let doc = Html::parse_document(&body);
        doc.root_element()
            .descendants()
            .filter_map(|n| {
                if let Node::Text(t) = n.value() {
                    (!n.ancestors().filter_map(ElementRef::wrap).any(|e| {
                        matches!(
                            e.value().name(),
                            "script" | "style" | "noscript" | "svg" | "iframe" | "head"
                        )
                    }))
                    .then(|| t.text.as_ref())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join(" ")
    };
    Ok(format!(
        "Public web page (untrusted):\n{}",
        crate::clip(&text, 12_000)
    ))
}
