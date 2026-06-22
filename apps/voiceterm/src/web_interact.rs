// src/web_interact.rs
// Web interaction layer: extract API endpoints, manage sessions,
// interact with chatbots and forms without JavaScript execution.
//
// Detects: Intercom, Drift, Zendesk, Crisp, Tawk.to, LiveChat,
// custom REST/WebSocket chat endpoints, CSRF tokens, OAuth flows.

use std::collections::HashMap;

/// Detected interactive element on a page
#[derive(Debug, Clone)]
pub enum WebWidget {
    /// Chat widget with API endpoint
    ChatWidget {
        provider: String,       // "intercom", "zendesk", "drift", "crisp", "tawk", "custom"
        endpoint: String,       // API URL
        app_id: Option<String>, // App/workspace ID
        config: HashMap<String, String>, // Extra config values
    },
    /// Login form with CSRF protection
    LoginForm {
        action: String,
        method: String,          // "POST" or "GET"
        username_field: String,
        password_field: String,
        csrf_token: Option<String>,
        csrf_field: Option<String>,
        extra_fields: Vec<(String, String)>, // hidden fields
    },
    /// API endpoint discovered in JS
    ApiEndpoint {
        url: String,
        method: String,
        auth_type: String, // "bearer", "cookie", "header", "none"
        description: String,
    },
    /// OAuth/SSO login link
    OAuthLink {
        provider: String, // "google", "github", "apple", "facebook", "microsoft"
        url: String,
    },
}

/// Scan HTML for interactive widgets
pub fn detect_widgets(html: &str, base_url: &str) -> Vec<WebWidget> {
    let mut widgets = Vec::new();
    let doc = scraper::Html::parse_document(html);

    detect_chat_widgets(html, base_url, &mut widgets);
    detect_login_forms(&doc, base_url, &mut widgets);
    detect_oauth_links(&doc, base_url, &mut widgets);
    detect_api_endpoints(html, &mut widgets);

    widgets
}

fn detect_chat_widgets(html: &str, _base_url: &str, widgets: &mut Vec<WebWidget>) {
    let lower = html.to_ascii_lowercase();

    // Intercom
    if lower.contains("intercom") {
        if let Some(app_id) = extract_between(html, "app_id", &['"', '\'']) {
            widgets.push(WebWidget::ChatWidget {
                provider: "intercom".into(), endpoint: format!("https://api-iam.intercom.io/messenger/web/ping?app_id={}", app_id),
                app_id: Some(app_id), config: HashMap::new(),
            });
        }
    }

    // Zendesk
    if lower.contains("zendesk") || lower.contains("zopim") {
        if let Some(_key) = extract_between(html, "zESettings", &['{']) {
            widgets.push(WebWidget::ChatWidget {
                provider: "zendesk".into(), endpoint: String::new(),
                app_id: None, config: HashMap::new(),
            });
        }
        // Zendesk chat widget script URL
        for pat in &["static.zdassets.com", "v2.zopim.com"] {
            if let Some(_pos) = lower.find(pat) {
                widgets.push(WebWidget::ChatWidget {
                    provider: "zendesk".into(), endpoint: format!("https://{}", pat),
                    app_id: None, config: HashMap::new(),
                });
                break;
            }
        }
    }

    // Drift
    if lower.contains("drift.com") || lower.contains("js.driftt.com") {
        if let Some(id) = extract_between(html, "drift.load", &['\'', '"']) {
            widgets.push(WebWidget::ChatWidget {
                provider: "drift".into(), endpoint: "https://event.api.drift.com".into(),
                app_id: Some(id), config: HashMap::new(),
            });
        }
    }

    // Crisp
    if lower.contains("crisp.chat") || lower.contains("client.crisp.chat") {
        if let Some(id) = extract_between(html, "CRISP_WEBSITE_ID", &['"', '\'']) {
            widgets.push(WebWidget::ChatWidget {
                provider: "crisp".into(), endpoint: format!("https://client.crisp.chat/l/{}/", id),
                app_id: Some(id), config: HashMap::new(),
            });
        }
    }

    // Tawk.to
    if lower.contains("tawk.to") || lower.contains("embed.tawk.to") {
        if let Some(id) = extract_between(html, "embed.tawk.to", &['/']) {
            widgets.push(WebWidget::ChatWidget {
                provider: "tawk".into(), endpoint: format!("https://embed.tawk.to/{}", id),
                app_id: Some(id), config: HashMap::new(),
            });
        }
    }

    // LiveChat
    if lower.contains("livechatinc.com") || lower.contains("cdn.livechatinc.com") {
        if let Some(license) = extract_between(html, "license", &['"', '\'', ':']) {
            let clean = license.trim_matches(|c: char| !c.is_ascii_digit());
            if !clean.is_empty() {
                widgets.push(WebWidget::ChatWidget {
                    provider: "livechat".into(), endpoint: "https://api.livechatinc.com".into(),
                    app_id: Some(clean.to_string()), config: HashMap::new(),
                });
            }
        }
    }
}

fn detect_login_forms(doc: &scraper::Html, base_url: &str, widgets: &mut Vec<WebWidget>) {
    static SEL: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
    let sel = SEL.get_or_init(|| scraper::Selector::parse("form").unwrap());

    for form in doc.select(sel) {
        let action = crate::qml_gen::resolve_url(form.value().attr("action").unwrap_or(""), base_url);
        let method = form.value().attr("method").unwrap_or("GET").to_uppercase();

        // Find password field — indicates login form
        let mut username_field = String::new();
        let mut password_field = String::new();
        let mut csrf_token = None;
        let mut csrf_field = None;
        let mut extra_fields = Vec::new();

        static SEL_INPUT: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
        let sel_inp = SEL_INPUT.get_or_init(|| scraper::Selector::parse("input").unwrap());

        for input in form.select(sel_inp) {
            let name = input.value().attr("name").unwrap_or("").to_string();
            let itype = input.value().attr("type").unwrap_or("text").to_ascii_lowercase();
            let value = input.value().attr("value").unwrap_or("").to_string();

            match itype.as_str() {
                "password" => { password_field = name; }
                "email" | "text" => {
                    let n = name.to_ascii_lowercase();
                    if n.contains("user") || n.contains("email") || n.contains("login") || n.contains("name") || n.contains("account") {
                        username_field = name;
                    }
                }
                "hidden" => {
                    let n = name.to_ascii_lowercase();
                    if n.contains("csrf") || n.contains("token") || n.contains("_token") || n.contains("authenticity") || n.contains("nonce") {
                        csrf_token = Some(value.clone());
                        csrf_field = Some(name.clone());
                    }
                    extra_fields.push((name, value));
                }
                _ => {}
            }
        }

        if !password_field.is_empty() {
            widgets.push(WebWidget::LoginForm {
                action, method, username_field, password_field,
                csrf_token, csrf_field, extra_fields,
            });
        }
    }
}

fn detect_oauth_links(doc: &scraper::Html, base_url: &str, widgets: &mut Vec<WebWidget>) {
    static SEL: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
    let sel = SEL.get_or_init(|| scraper::Selector::parse("a[href]").unwrap());

    let oauth_patterns = [
        ("google", &["accounts.google.com/o/oauth", "accounts.google.com/signin"] as &[&str]),
        ("github", &["github.com/login/oauth"]),
        ("apple", &["appleid.apple.com/auth"]),
        ("facebook", &["facebook.com/v", "facebook.com/dialog/oauth"]),
        ("microsoft", &["login.microsoftonline.com", "login.live.com"]),
    ];

    for link in doc.select(sel) {
        let href = link.value().attr("href").unwrap_or("");
        let lower = href.to_ascii_lowercase();
        for (provider, patterns) in &oauth_patterns {
            if patterns.iter().any(|p| lower.contains(p)) {
                widgets.push(WebWidget::OAuthLink {
                    provider: provider.to_string(),
                    url: crate::qml_gen::resolve_url(href, base_url),
                });
                break;
            }
        }
    }
}

fn detect_api_endpoints(html: &str, widgets: &mut Vec<WebWidget>) {
    // Look for REST API base URLs in scripts
    let api_patterns = [
        ("api/v", "REST API"),
        ("graphql", "GraphQL"),
        ("/api/", "REST API"),
        ("api.stripe.com", "Stripe Payments"),
        ("api.plaid.com", "Plaid Banking"),
        ("api.braintreegateway.com", "Braintree Payments"),
    ];

    for (pattern, desc) in &api_patterns {
        let mut pos = 0;
        while pos < html.len() && widgets.len() < 20 {
            if let Some(found) = html[pos..].find(pattern) {
                let start = pos + found;
                // Try to extract the full URL
                let before = &html[start.saturating_sub(100)..start];
                if let Some(url_start) = before.rfind(|c: char| c == '"' || c == '\'' || c == '`') {
                    let url = &html[start.saturating_sub(100) + url_start + 1..];
                    if let Some(url_end) = url.find(|c: char| c == '"' || c == '\'' || c == '`' || c == ' ') {
                        let full_url = &url[..url_end];
                        if full_url.starts_with("http") || full_url.starts_with("/") {
                            widgets.push(WebWidget::ApiEndpoint {
                                url: full_url.to_string(), method: "GET".into(),
                                auth_type: "unknown".into(), description: desc.to_string(),
                            });
                        }
                    }
                }
                pos = start + pattern.len();
            } else { break; }
        }
    }
}

/// Submit a login form
pub fn submit_login(agent: &ureq::Agent, form: &WebWidget, username: &str, password: &str) -> Result<String, String> {
    if let WebWidget::LoginForm { action, method, username_field, password_field, csrf_token, csrf_field, extra_fields } = form {
        let mut params = Vec::new();
        params.push((username_field.as_str(), username));
        params.push((password_field.as_str(), password));
        if let (Some(field), Some(token)) = (csrf_field, csrf_token) {
            params.push((field.as_str(), token.as_str()));
        }
        for (k, v) in extra_fields {
            params.push((k.as_str(), v.as_str()));
        }

        if method == "POST" {
            let body: String = params.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&");
            let resp = agent.post(action)
                .set("Content-Type", "application/x-www-form-urlencoded")
                .send_string(&body)
                .map_err(|e| format!("Login error: {}", e))?;
            resp.into_string().map_err(|e| format!("Read error: {}", e))
        } else {
            let query: String = params.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&");
            let url = format!("{}?{}", action, query);
            let resp = agent.get(&url).call().map_err(|e| format!("Login error: {}", e))?;
            resp.into_string().map_err(|e| format!("Read error: {}", e))
        }
    } else { Err("Not a login form".into()) }
}

fn url_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 2);
    for b in s.bytes() { match b {
        b'A'..=b'Z'|b'a'..=b'z'|b'0'..=b'9'|b'-'|b'_'|b'.'|b'~' => o.push(b as char),
        b' ' => o.push('+'), _ => { o.push('%'); o.push(char::from(b"0123456789ABCDEF"[(b>>4) as usize])); o.push(char::from(b"0123456789ABCDEF"[(b&0xf) as usize])); }
    } }
    o
}

/// Format detected widgets for QML display
pub fn format_widgets_for_qml(widgets: &[WebWidget]) -> String {
    let mut out = String::with_capacity(widgets.len() * 100);
    for w in widgets {
        match w {
            WebWidget::ChatWidget { provider, endpoint, app_id, .. } => {
                out.push_str("W\x1Fchat\x1F"); out.push_str(provider);
                out.push('\x1F'); out.push_str(app_id.as_deref().unwrap_or(""));
                out.push('\x1F'); out.push_str(endpoint);
                out.push('\n');
            }
            WebWidget::LoginForm { action, username_field, password_field, csrf_token, .. } => {
                out.push_str("W\x1Flogin\x1F"); out.push_str(action);
                out.push('\x1F'); out.push_str(username_field);
                out.push('\x1F'); out.push_str(password_field);
                out.push('\x1F'); out.push_str(if csrf_token.is_some() { "csrf" } else { "none" });
                out.push('\n');
            }
            WebWidget::OAuthLink { provider, url } => {
                out.push_str("W\x1Foauth\x1F"); out.push_str(provider);
                out.push('\x1F'); out.push_str(url);
                out.push('\n');
            }
            WebWidget::ApiEndpoint { url, description, .. } => {
                out.push_str("W\x1Fapi\x1F"); out.push_str(description);
                out.push('\x1F'); out.push_str(url);
                out.push('\n');
            }
        }
    }
    out
}

fn extract_between(html: &str, key: &str, delimiters: &[char]) -> Option<String> {
    let pos = html.find(key)?;
    let rest = &html[pos + key.len()..html.len().min(pos + key.len() + 200)];
    // Skip to first delimiter
    for &d in delimiters {
        if let Some(start) = rest.find(d) {
            let after = &rest[start + 1..];
            if let Some(end) = after.find(d) {
                let val = &after[..end];
                if !val.is_empty() && val.len() < 100 { return Some(val.to_string()); }
            }
        }
    }
    None
}
