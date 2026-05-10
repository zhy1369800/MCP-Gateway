use std::path::{Component, Path, PathBuf};

use axum::extract::Path as AxumPath;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

const WELCOME_ROOT: &str = "/data/www";

pub fn router() -> Router {
    Router::new()
        .route("/", get(get_index))
        .route("/*path", get(get_static_asset))
}

async fn get_index() -> Response {
    match tokio::fs::read_to_string(format!("{WELCOME_ROOT}/index.html")).await {
        Ok(contents) => Html(contents).into_response(),
        Err(_) => Html(default_index_html()).into_response(),
    }
}

async fn get_static_asset(AxumPath(path): AxumPath<String>) -> Response {
    let Ok(safe_path) = sanitize_relative_path(&path) else {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    };

    let base_root = Path::new(WELCOME_ROOT);
    let candidate = if safe_path.extension().is_none() {
        let html_path = base_root.join(format!("{}.html", safe_path.to_string_lossy()));
        if tokio::fs::metadata(&html_path).await.is_ok() {
            html_path
        } else {
            base_root.join(&safe_path)
        }
    } else {
        base_root.join(&safe_path)
    };

    match tokio::fs::read(&candidate).await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, content_type_for(&candidate));
            (headers, bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn sanitize_relative_path(input: &str) -> Result<PathBuf, ()> {
    let candidate = PathBuf::from(input.trim_start_matches('/'));
    if candidate.as_os_str().is_empty() {
        return Err(());
    }

    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(()),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(());
    }

    Ok(normalized)
}

fn content_type_for(path: &Path) -> header::HeaderValue {
    let mime = match path.extension().and_then(|ext| ext.to_str()).unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    header::HeaderValue::from_static(mime)
}

fn default_index_html() -> &'static str {
    r#"<!doctype html>
<html lang=\"en\">
<head>
  <meta charset=\"utf-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
  <title>MCP Gateway</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #0b1020;
      --panel: rgba(19, 27, 50, 0.82);
      --panel-border: rgba(148, 163, 184, 0.18);
      --text: #e5eefc;
      --muted: #9fb0d0;
      --accent: #7c3aed;
      --accent-2: #06b6d4;
      --ok: #22c55e;
      --shadow: 0 30px 80px rgba(0, 0, 0, 0.45);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif;
      background:
        radial-gradient(circle at top left, rgba(124, 58, 237, 0.32), transparent 34%),
        radial-gradient(circle at top right, rgba(6, 182, 212, 0.26), transparent 28%),
        linear-gradient(180deg, #0b1020 0%, #111827 100%);
      color: var(--text);
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 32px;
    }
    .card {
      width: min(920px, 100%);
      border: 1px solid var(--panel-border);
      background: var(--panel);
      backdrop-filter: blur(14px);
      border-radius: 24px;
      box-shadow: var(--shadow);
      overflow: hidden;
    }
    .hero {
      padding: 40px 40px 28px;
      border-bottom: 1px solid rgba(148, 163, 184, 0.12);
    }
    .badge {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 8px 12px;
      border-radius: 999px;
      background: rgba(34, 197, 94, 0.12);
      color: #d1fae5;
      font-size: 13px;
      font-weight: 600;
      letter-spacing: 0.02em;
    }
    .badge::before {
      content: \"\";
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: var(--ok);
      box-shadow: 0 0 0 6px rgba(34, 197, 94, 0.18);
    }
    h1 {
      margin: 18px 0 12px;
      font-size: clamp(32px, 6vw, 52px);
      line-height: 1.02;
      letter-spacing: -0.04em;
    }
    p.lead {
      margin: 0;
      max-width: 760px;
      color: var(--muted);
      font-size: 18px;
      line-height: 1.7;
    }
    .grid {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
      gap: 18px;
      padding: 28px 40px 40px;
    }
    .item {
      border: 1px solid rgba(148, 163, 184, 0.14);
      border-radius: 18px;
      padding: 18px 18px 16px;
      background: rgba(15, 23, 42, 0.58);
    }
    .item h2 {
      margin: 0 0 10px;
      font-size: 14px;
      color: #cbd5e1;
      font-weight: 700;
      letter-spacing: 0.04em;
      text-transform: uppercase;
    }
    .item p {
      margin: 0;
      color: var(--muted);
      line-height: 1.7;
      font-size: 14px;
    }
    code {
      display: inline-block;
      margin-top: 8px;
      padding: 4px 8px;
      border-radius: 10px;
      background: rgba(2, 6, 23, 0.82);
      color: #dbeafe;
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, \"Liberation Mono\", monospace;
      font-size: 13px;
      word-break: break-all;
    }
    .footer {
      padding: 0 40px 36px;
      color: #93a4c3;
      font-size: 13px;
    }
  </style>
</head>
<body>
  <section class=\"card\">
    <div class=\"hero\">
      <span class=\"badge\">Gateway Online</span>
      <h1>Hello World</h1>
      <p class=\"lead\">MCP Gateway is running successfully. This landing page is served from the backend and can be overridden at any time by mounting your own static page.</p>
    </div>
    <div class=\"grid\">
      <article class=\"item\">
        <h2>Custom page</h2>
        <p>Mount your own welcome page here:<br /><code>/data/www/index.html</code></p>
      </article>
      <article class=\"item\">
        <h2>Health check</h2>
        <p>Use the admin health endpoint to verify the gateway status:<br /><code>/api/v2/admin/health</code></p>
      </article>
      <article class=\"item\">
        <h2>Skill roots</h2>
        <p>Recommended persistent directory for remote skills:<br /><code>/data/skills</code></p>
      </article>
    </div>
    <div class=\"footer\">If you replace <code>/data/www/index.html</code>, your custom page will be served automatically on the next request.</div>
  </section>
</body>
</html>"#
}
