use axum::http::{HeaderValue, Response, header::CONTENT_TYPE};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use if_addrs::get_if_addrs;
use percent_encoding::percent_decode_str;
use std::net::IpAddr;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::Arc;

/// Simple HTTP file server
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Root directory to serve files from
    #[arg(default_value = ".")]
    root: PathBuf,

    /// Client-side routing mode: return index.html for not found paths
    #[arg(long)]
    csr: bool,
}

/// Shared application state.
#[derive(Clone)]
struct AppState {
    root_dir: PathBuf,
    csr: bool,
}

// helper that returns a content-type for a given filename/path using the
// mime_guess crate. it will fall back to `application/octet-stream`
// automatically when the extension is unknown.
fn content_type_for(path: &StdPath) -> mime::Mime {
    mime_guess::from_path(path).first_or_octet_stream()
}

/// Build an HTTP response with the given bytes and content-type derived from
/// the file extension.
fn file_response(contents: Vec<u8>, file_path: &StdPath) -> Response<Body> {
    let mime = content_type_for(file_path);
    let mut resp = Response::new(Body::from(contents));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap());
    resp
}

/// Handler that serves files from the configured root directory.
///
/// Prevents path traversal by rejecting `..` components and ignores any prefix or
/// root segment. If the requested path is a directory or empty, `index.html` is
/// appended automatically.
///
/// When `--csr` is enabled and the requested file is not found, the server
/// returns the root `index.html` instead of a 404 — this is useful for
/// single-page applications that handle routing on the client side.
async fn serve_file_impl(root_dir: PathBuf, csr: bool, req_path: String) -> impl IntoResponse {
    // log raw request path so we can trace what the server receives
    println!("requested path: {}", req_path);
    // decode percent-encoding in case paths contain spaces or other encoded chars
    let decoded = match percent_decode_str(&req_path).decode_utf8() {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let mut fs_path = std::path::PathBuf::new();

    if decoded.is_empty() {
        // root request
        fs_path.push("index.html");
    } else {
        for comp in StdPath::new(&*decoded).components() {
            match comp {
                Component::Prefix(_) | Component::RootDir => continue,
                Component::ParentDir => {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
                Component::Normal(os_str) => fs_path.push(os_str),
                _ => {}
            }
        }

        if fs_path.as_os_str().is_empty() {
            fs_path.push("index.html");
        }
    }

    let mut full_path = root_dir.join(&fs_path);

    // if path still refers to a directory, serve its index
    if full_path.is_dir() {
        full_path.push("index.html");
    }

    match tokio::fs::read(&full_path).await {
        Ok(contents) => file_response(contents, &full_path).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if csr {
                // CSR mode: fall back to root index.html for SPA client-side routing
                let index_path = root_dir.join("index.html");
                match tokio::fs::read(&index_path).await {
                    Ok(contents) => file_response(contents, &index_path).into_response(),
                    Err(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
                    }
                }
            } else {
                (StatusCode::NOT_FOUND, "Not Found").into_response()
            }
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
    }
}

async fn serve_file(
    State(state): State<Arc<AppState>>,
    Path(req_path): Path<String>,
) -> impl IntoResponse {
    serve_file_impl(state.root_dir.clone(), state.csr, req_path).await
}

async fn serve_root(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    serve_file_impl(state.root_dir.clone(), state.csr, String::new()).await
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let root_dir = if args.root.as_os_str() == "." {
        std::env::current_dir().expect("failed to determine current directory")
    } else {
        args.root.clone()
    };

    if !root_dir.exists() {
        panic!("root directory does not exist: {}", root_dir.display());
    }

    if !root_dir.is_dir() {
        panic!("root path is not a directory: {}", root_dir.display());
    }

    let root_dir = root_dir
        .canonicalize()
        .expect("failed to resolve root directory");

    let state = Arc::new(AppState {
        root_dir: root_dir.clone(),
        csr: args.csr,
    });

    // bind to an OS-assigned ephemeral port
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("failed to bind to address");
    let addr = listener.local_addr().expect("unable to get local addr");
    let port = addr.port();

    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            match iface.ip() {
                IpAddr::V4(ip) if !ip.is_loopback() => {
                    println!("Listening on http://{}:{}", ip, port);
                }
                IpAddr::V6(ip) if !ip.is_loopback() => {
                    println!("Listening on http://[{}]:{}", ip, port);
                }
                _ => {}
            }
        }
    }
    println!("Serving files from {}", root_dir.display());
    if args.csr {
        println!("CSR mode enabled — not-found paths return index.html");
    }

    let app = Router::new()
        .route("/{*path}", get(serve_file))
        .route("/", get(serve_root))
        .with_state(state);

    axum::serve(listener, app).await.expect("server error");
}
