use axum::{Router, extract::Path, http::StatusCode, response::IntoResponse, routing::get};
// bring some http helpers axum re‑exports so we don't have to depend on the
// standalone `http` crate directly
use axum::http::{HeaderValue, Response, header::CONTENT_TYPE};

use if_addrs::get_if_addrs;
use percent_encoding::percent_decode_str;
use std::net::IpAddr;
use std::path::{Component, Path as StdPath};

// helper that returns a content-type for a given filename/path using the
// mime_guess crate. it will fall back to `application/octet-stream`
// automatically when the extension is unknown.
fn content_type_for(path: &StdPath) -> mime::Mime {
    mime_guess::from_path(path).first_or_octet_stream()
}

/// Handler that serves files from the current directory.
///
/// Prevents path traversal by rejecting `..` components and ignores any prefix or
/// root segment. If the requested path is a directory or empty, `index.html` is
/// appended automatically.
async fn serve_file(Path(req_path): Path<String>) -> impl IntoResponse {
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

    // if path still refers to a directory, serve its index
    if fs_path.is_dir() {
        fs_path.push("index.html");
    }

    match tokio::fs::read(&fs_path).await {
        Ok(contents) => {
            // look up mime based on the file extension using the helper (which uses mime_guess)
            let mime = content_type_for(&fs_path);
            // build a response with the appropriate Content-Type header
            let mut resp = Response::new(contents.into());
            *resp.status_mut() = StatusCode::OK;
            resp.headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap());
            resp
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
    }
}

#[tokio::main]
async fn main() {
    // bind to an OS-assigned ephemeral port
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("failed to bind to address");
    let addr = listener.local_addr().expect("unable to get local addr");
    // the listener itself is bound to 0.0.0.0 (all interfaces), so the
    // `addr` returned here will usually be 0.0.0.0:<port>. consumers want to
    // know which IPs on the host they can reach, though, so enumerate the
    // local interfaces and print a line for each.
    let port = addr.port();
    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            // ignore the loopback address – people don't normally want to hit
            // the server via 127.0.0.1/::1 from another machine.
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

    let app = Router::new().route("/{*path}", get(serve_file));

    // use the convenience helper from axum to run the app
    axum::serve(listener, app).await.expect("server error");
}
