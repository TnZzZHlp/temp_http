use axum::{Router, extract::Path, http::StatusCode, response::IntoResponse, routing::get};
use percent_encoding::percent_decode_str;
use std::path::{Component, Path as StdPath};

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
        Ok(contents) => (StatusCode::OK, contents).into_response(),
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
    println!("Listening on http://{}", addr);

    let app = Router::new().route("/{*path}", get(serve_file));

    // use the convenience helper from axum to run the app
    axum::serve(listener, app).await.expect("server error");
}
