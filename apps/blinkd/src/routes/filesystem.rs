//! `/api/v1/filesystem` — per-server file manager (browser-facing).
//!
//! Every request is token-authenticated against the panel by the
//! `require_panel_token` middleware, which stores the validated server in the
//! request extensions. `server_id` is always the first path segment and the
//! token's validated `internalId` is used as the volume root.

use crate::error::{AppError, AppResult};
use crate::models::{CargoFile, CargoProperties, ValidateResponse};
use crate::util;
use crate::AppState;
use axum::body::Body;
use axum::extract::multipart::Multipart;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio_util::io::ReaderStream;
use walkdir::WalkDir;

/// Canonical volume root for the validated server.
fn server_root(state: &AppState, internal_id: &str) -> PathBuf {
    let canonical_base = std::fs::canonicalize(state.config.volumes_directory.trim())
        .unwrap_or_else(|_| PathBuf::from(&state.config.volumes_directory));
    canonical_base.join(util::sanitize_volume_name(internal_id))
}

/// Resolve a request path within the server volume (lexical + symlink safe).
fn resolve(state: &AppState, internal_id: &str, request_path: &str) -> AppResult<PathBuf> {
    let root = server_root(state, internal_id);
    if !root.exists() {
        return Err(AppError::not_found("Server volume not found"));
    }
    util::resolve_within(&root, request_path).ok_or_else(|| AppError::bad_request("Invalid path"))
}

fn query_map(raw: &Option<String>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = raw {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                let key = percent_encoding::percent_decode_str(k).decode_utf8_lossy().to_string();
                let value =
                    percent_encoding::percent_decode_str(v).decode_utf8_lossy().to_string();
                map.insert(key, value);
            }
        }
    }
    map
}

/// Cargo metadata for one file (relative path), fetched live from the panel.
async fn cargo_properties(
    state: &AppState,
    internal_id: &str,
    token: &str,
    relative_path: &str,
) -> (bool, Option<CargoProperties>) {
    let files = state
        .panel
        .fetch_cargo_files(internal_id, token)
        .await
        .cargo_files;
    let normalized = relative_path.trim_start_matches('/');
    for cargo in files {
        if cargo.target_path.trim_start_matches('/') == normalized {
            return (true, Some(cargo.properties));
        }
    }
    (false, None)
}

fn file_stats_json(
    name: &str,
    meta: &std::fs::Metadata,
    is_cargo: bool,
    props: Option<&CargoProperties>,
) -> serde_json::Value {
    use std::os::unix::fs::MetadataExt;
    let is_file = meta.is_file();
    let is_symlink = meta.file_type().is_symlink();
    let mode = format!("{:04o}", meta.mode() & 0o7777);
    let mime = if is_file {
        mime_guess::from_path(name).first_or_octet_stream().to_string()
    } else {
        "inode/directory".to_string()
    };
    json!({
        "name": name,
        "mode": mode,
        "size": meta.len(),
        "isFile": is_file,
        "isSymlink": is_symlink,
        "modifiedAt": meta.mtime() as u128 * 1000,
        "createdAt": meta.ctime() as u128 * 1000,
        "mime": mime,
        "isCargoFile": is_cargo,
        "hidden": props.map(|p| p.is_hidden()).unwrap_or(false),
        "readonly": props.map(|p| p.is_readonly()).unwrap_or(false),
        "noDelete": props.map(|p| p.is_no_delete()).unwrap_or(false),
        "customProperties": props.and_then(|p| p.custom_properties.clone()).unwrap_or(serde_json::Value::Null)
    })
}

fn extract_bearer(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

async fn list_impl(
    state: &Arc<AppState>,
    validation: &ValidateResponse,
    token: &str,
    request_path: &str,
    raw_query: Option<String>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let full_path = resolve(state, &internal_id, request_path)?;

    let meta = tokio::fs::metadata(&full_path).await?;
    if !meta.is_dir() {
        return Err(AppError::bad_request("Path is not a directory"));
    }

    let query = query_map(&raw_query);
    let show_hidden = query.get("showHidden").map(|v| v == "true").unwrap_or(false);

    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, token)
        .await
        .cargo_files;

    let mut entries = tokio::fs::read_dir(&full_path).await?;
    let mut contents = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(entry_meta) = entry.metadata().await else { continue };
        let relative = StdPath::new(request_path)
            .join(&name)
            .to_string_lossy()
            .trim_start_matches('/')
            .to_string();

        let cargo_match = cargo_files
            .iter()
            .find(|c| c.target_path.trim_start_matches('/') == relative);
        let props = cargo_match.map(|c| c.properties.clone());

        if !show_hidden && props.as_ref().map(|p| p.is_hidden()).unwrap_or(false) {
            continue;
        }

        contents.push(file_stats_json(
            &name,
            &entry_meta,
            cargo_match.is_some(),
            props.as_ref(),
        ));
    }

    Ok(Json(json!({ "contents": contents })).into_response())
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    list_impl(&state, &validation, &extract_bearer(&headers), &path, raw_query).await
}

pub async fn list_root(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Path(_server_id): Path<String>,
) -> AppResult<Response> {
    list_impl(&state, &validation, &extract_bearer(&headers), ".", raw_query).await
}

// ---------------------------------------------------------------------------
// contents
// ---------------------------------------------------------------------------

pub async fn contents(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let full_path = resolve(&state, &internal_id, &path)?;
    let meta = tokio::fs::metadata(&full_path).await?;
    if !meta.is_file() {
        return Err(AppError::bad_request("Path is not a file"));
    }
    if meta.len() > 100 * 1024 * 1024 {
        return Err(AppError::PayloadTooLarge("File too large to read".into()));
    }

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &path).await;

    let file = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file);
    let mut builder = Response::builder()
        .header(
            header::CONTENT_TYPE,
            mime_guess::from_path(&full_path).first_or_octet_stream().to_string(),
        )
        .header(header::CONTENT_LENGTH, meta.len());

    if is_cargo {
        builder = builder
            .header("X-Cargo-File", "true")
            .header(
                "X-Cargo-Readonly",
                if props.as_ref().map(|p| p.is_readonly()).unwrap_or(false) {
                    "true"
                } else {
                    "false"
                },
            );
    }

    Ok(builder.body(Body::from_stream(stream)).unwrap().into_response())
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

pub async fn write(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, path)): Path<(String, String)>,
    body: Body,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let full_path = resolve(&state, &internal_id, &path)?;

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &path).await;
    if is_cargo && props.as_ref().map(|p| p.is_readonly()).unwrap_or(false) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Cannot modify readonly cargo file", "cargoFile": true })),
        )
            .into_response());
    }

    let bytes = axum::body::to_bytes(body, 100 * 1024 * 1024)
        .await
        .map_err(|_| AppError::PayloadTooLarge("Body exceeds 100MB".into()))?;

    if let Some(parent) = full_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&full_path, &bytes).await?;

    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

pub async fn delete_path(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let full_path = resolve(&state, &internal_id, &path)?;

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &path).await;
    if is_cargo && props.as_ref().map(|p| p.is_no_delete()).unwrap_or(false) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Cannot delete protected cargo file", "cargoFile": true })),
        )
            .into_response());
    }

    let meta = tokio::fs::symlink_metadata(&full_path).await?;
    if meta.is_dir() {
        // Refuse to delete a directory containing protected cargo files.
        let cargo_files = state
            .panel
            .fetch_cargo_files(&internal_id, &token)
            .await
            .cargo_files;
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let protected: Vec<&str> = cargo_files
            .iter()
            .filter(|c| {
                (c.properties.is_readonly() || c.properties.is_no_delete())
                    && c.target_path.trim_start_matches('/').starts_with(&prefix)
            })
            .map(|c| c.target_path.as_str())
            .collect();
        if !protected.is_empty() {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "Directory contains protected cargo files",
                    "protectedFiles": protected
                })),
            )
                .into_response());
        }
        tokio::fs::remove_dir_all(&full_path).await?;
    } else {
        tokio::fs::remove_file(&full_path).await?;
    }

    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// create-directory
// ---------------------------------------------------------------------------

pub async fn create_directory(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();

    let full_path = resolve(&state, &internal_id, &path)?;
    tokio::fs::create_dir_all(&full_path).await?;
    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// rename
// ---------------------------------------------------------------------------

pub async fn rename(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path(_server_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let from = body.get("from").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let to = body.get("to").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if from.is_empty() || to.is_empty() {
        return Err(AppError::bad_request("from and to are required"));
    }

    let from_path = resolve(&state, &internal_id, &from)?;
    let to_path = resolve(&state, &internal_id, &to)?;

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &from).await;
    if is_cargo
        && (props.as_ref().map(|p| p.is_readonly()).unwrap_or(false)
            || props.as_ref().map(|p| p.is_no_delete()).unwrap_or(false))
    {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Cannot move protected cargo file", "cargoFile": true })),
        )
            .into_response());
    }

    if tokio::fs::metadata(&from_path).await?.is_dir() {
        let cargo_files = state
            .panel
            .fetch_cargo_files(&internal_id, &token)
            .await
            .cargo_files;
        let prefix = format!("{}/", from.trim_end_matches('/'));
        let protected: Vec<String> = cargo_files
            .iter()
            .filter(|c| {
                (c.properties.is_readonly() || c.properties.is_no_delete())
                    && c.target_path.trim_start_matches('/').starts_with(&prefix)
            })
            .map(|c| c.target_path.clone())
            .collect();
        if !protected.is_empty() {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "Directory contains protected cargo files",
                    "protectedFiles": protected
                })),
            )
                .into_response());
        }
    }

    tokio::fs::rename(&from_path, &to_path).await?;
    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

pub async fn copy(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    Path(_server_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();

    let from = body.get("from").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let to = body.get("to").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if from.is_empty() || to.is_empty() {
        return Err(AppError::bad_request("from and to are required"));
    }

    let from_path = resolve(&state, &internal_id, &from)?;
    let to_path = resolve(&state, &internal_id, &to)?;

    if tokio::fs::metadata(&from_path).await?.is_dir() {
        copy_dir_recursive(&from_path, &to_path).await?;
    } else {
        tokio::fs::copy(&from_path, &to_path).await?;
    }

    Ok(Json(json!({ "success": true })).into_response())
}

fn copy_dir_recursive<'a>(src: &'a StdPath, dst: &'a StdPath) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<()>> + Send + 'a>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(dst).await?;
        let mut entries = tokio::fs::read_dir(src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let target = dst.join(entry.file_name());
            if entry.file_type().await?.is_dir() {
                copy_dir_recursive(&entry.path(), &target).await?;
            } else {
                tokio::fs::copy(entry.path(), target).await?;
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// chmod
// ---------------------------------------------------------------------------

pub async fn chmod(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path(_server_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let target_path = body.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if target_path.is_empty() {
        return Err(AppError::bad_request("path is required"));
    }
    let mode = match body.get("mode") {
        Some(serde_json::Value::String(s)) => u32::from_str_radix(s, 8)
            .map_err(|_| AppError::bad_request("invalid mode"))?,
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .filter(|m| *m <= 0o7777)
            .ok_or_else(|| AppError::bad_request("invalid mode"))? as u32,
        _ => return Err(AppError::bad_request("mode is required")),
    };

    let full_path = resolve(&state, &internal_id, &target_path)?;

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &target_path).await;
    if is_cargo && props.as_ref().map(|p| p.is_readonly()).unwrap_or(false) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Cannot change permissions on readonly cargo file",
                "cargoFile": true
            })),
        )
            .into_response());
    }

    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(&full_path, std::fs::Permissions::from_mode(mode)).await?;
    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// upload
// ---------------------------------------------------------------------------

pub async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, target_dir)): Path<(String, String)>,
    multipart: Multipart,
) -> AppResult<Response> {
    upload_impl(&state, &validation, &extract_bearer(&headers), &target_dir, multipart).await
}

pub async fn upload_root(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path(_server_id): Path<String>,
    multipart: Multipart,
) -> AppResult<Response> {
    upload_impl(&state, &validation, &extract_bearer(&headers), ".", multipart).await
}

async fn upload_impl(
    state: &Arc<AppState>,
    validation: &ValidateResponse,
    token: &str,
    target_dir: &str,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();

    let full_target_dir = resolve(state, &internal_id, target_dir)?;
    if !full_target_dir.is_dir() {
        return Err(AppError::bad_request("Invalid target directory"));
    }

    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, token)
        .await
        .cargo_files;

    let mut results = Vec::new();
    while let Some(field) = multipart.next_field().await? {
        let field_name = field.name().unwrap_or_default().to_string();
        if field_name != "files" && field_name != "file" {
            continue;
        }
        let original_name = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("upload-{}", uuid::Uuid::new_v4()));
        // Never let a client-supplied filename escape the target directory.
        let file_name = util::sanitize_volume_name(&original_name);
        if file_name.is_empty() || file_name.starts_with('.') {
            results.push(json!({
                "name": original_name,
                "success": false,
                "error": "Invalid file name"
            }));
            continue;
        }

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::bad_request(format!("upload failed: {e}")))?;

        let target_path = full_target_dir.join(&file_name);
        let relative = target_path
            .strip_prefix(server_root(state, &internal_id))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let conflict = cargo_files
            .iter()
            .find(|c| c.target_path.trim_start_matches('/') == relative && c.properties.is_readonly());

        if conflict.is_some() {
            results.push(json!({
                "name": original_name,
                "success": false,
                "error": "Cannot overwrite readonly cargo file"
            }));
            continue;
        }

        match tokio::fs::write(&target_path, &data).await {
            Ok(()) => results.push(json!({ "name": original_name, "success": true })),
            Err(e) => results.push(json!({
                "name": original_name,
                "success": false,
                "error": e.to_string()
            })),
        }
    }

    Ok(Json(json!({ "files": results })).into_response())
}

// ---------------------------------------------------------------------------
// download
// ---------------------------------------------------------------------------

pub async fn download(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    download_impl(&state, &validation, &path).await
}

pub async fn download_root(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    Path(_server_id): Path<String>,
) -> AppResult<Response> {
    download_impl(&state, &validation, ".").await
}

async fn download_impl(
    state: &Arc<AppState>,
    validation: &ValidateResponse,
    path: &str,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();

    let full_path = resolve(state, &internal_id, path)?;

    // Build the archive in a temp file, then stream it out.
    let temp_file = std::env::temp_dir().join(format!("blinkd-download-{}.zip", uuid::Uuid::new_v4()));
    let root_name = path.rsplit('/').next().unwrap_or("download").to_string();
    create_zip(&full_path, &temp_file, &root_name)?;

    let file = tokio::fs::File::open(&temp_file).await?;
    let size = file.metadata().await?.len();
    let stream = ReaderStream::new(file);

    let name = util::sanitize_volume_name(&root_name);
    let response = Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.zip\""))
        .header(header::CONTENT_LENGTH, size)
        .body(Body::from_stream(stream))
        .unwrap();

    // Delete the temp archive five minutes from now (generous drain window).
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        let _ = tokio::fs::remove_file(&temp_file).await;
    });

    Ok(response.into_response())
}

fn create_zip(source: &StdPath, dest: &StdPath, root_name: &str) -> AppResult<()> {
    let meta = std::fs::metadata(source)?;
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    if meta.is_file() {
        zip.start_file(root_name, options)
            .map_err(|e| AppError::internal(format!("zip failed: {e}")))?;
        let mut content = std::fs::File::open(source)?;
        std::io::copy(&mut content, &mut zip)?;
    } else {
        for entry in WalkDir::new(source)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let rel = entry
                .path()
                .strip_prefix(source)
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let rel_str = rel.to_string_lossy();
            if entry.file_type().is_dir() {
                if !rel_str.is_empty() {
                    zip.add_directory(rel_str, options)
                        .map_err(|e| AppError::internal(format!("zip failed: {e}")))?;
                }
            } else {
                zip.start_file(rel_str, options)
                    .map_err(|e| AppError::internal(format!("zip failed: {e}")))?;
                let mut content = std::fs::File::open(entry.path())?;
                std::io::copy(&mut content, &mut zip)?;
            }
        }
    }

    zip.finish()
        .map_err(|e| AppError::internal(format!("zip failed: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

pub async fn extract(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, path)): Path<(String, String)>,
    multipart: Multipart,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let full_target_dir = resolve(&state, &internal_id, &path)?;
    if !full_target_dir.is_dir() {
        return Err(AppError::bad_request("Invalid target directory"));
    }

    let mut multipart = multipart;
    let mut archive_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("file") {
            archive_bytes = Some(
                field
                    .bytes()
                    .await
                    .map_err(|e| AppError::bad_request(format!("upload failed: {e}")))?
                    .to_vec(),
            );
            break;
        }
    }
    let archive_bytes = archive_bytes.ok_or_else(|| AppError::bad_request("No file provided"))?;

    // Extract into a temp dir first so we can check cargo conflicts.
    let temp_dir = std::env::temp_dir().join(format!("blinkd-extract-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await?;

    let unzip_result: AppResult<()> = (|| {
        let reader = std::io::Cursor::new(&archive_bytes);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| AppError::bad_request(format!("invalid zip archive: {e}")))?;
        // ZipArchive::extract rejects entries escaping the destination.
        archive
            .extract(&temp_dir)
            .map_err(|e| AppError::bad_request(format!("extraction failed: {e}")))?;
        Ok(())
    })();

    if let Err(e) = unzip_result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    // Conflict check against readonly cargo files.
    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, &token)
        .await
        .cargo_files;
    let readonly: Vec<&CargoFile> =
        cargo_files.iter().filter(|c| c.properties.is_readonly()).collect();

    if !readonly.is_empty() {
        let mut extracted_files: Vec<String> = Vec::new();
        for entry in WalkDir::new(&temp_dir).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(&temp_dir).unwrap_or(entry.path());
                let joined = StdPath::new(&path).join(rel);
                extracted_files.push(joined.to_string_lossy().trim_start_matches("./").to_string());
            }
        }
        for cargo in &readonly {
            for extracted in &extracted_files {
                if cargo.target_path.trim_start_matches('/') == extracted.trim_start_matches('/') {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Ok((
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": format!(
                                "Extraction would overwrite readonly cargo file: {}",
                                cargo.target_path
                            )
                        })),
                    )
                        .into_response());
                }
            }
        }
    }

    let copy_result = copy_dir_recursive(&temp_dir, &full_target_dir).await;
    let _ = std::fs::remove_dir_all(&temp_dir);
    copy_result?;

    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// disk-usage / search / compress / cargo-info / cargo-files
// ---------------------------------------------------------------------------

pub async fn disk_usage(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    Path(_server_id): Path<String>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();

    let server_path = server_root(&state, &internal_id);
    if !server_path.exists() {
        return Err(AppError::not_found("Server volume not found"));
    }

    let mut total_size: u64 = 0;
    let mut file_count: u64 = 0;
    for entry in WalkDir::new(&server_path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                total_size += meta.len();
                file_count += 1;
            }
        }
    }

    Ok(Json(json!({
        "bytes": total_size,
        "files": file_count,
        "human": format!("{:.2} MB", total_size as f64 / (1024.0 * 1024.0))
    }))
    .into_response())
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Path(_server_id): Path<String>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let query = query_map(&raw_query);
    let search_query = query.get("query").cloned().unwrap_or_default();
    let search_path = query.get("path").cloned().unwrap_or_else(|| ".".into());
    let show_hidden = query.get("showHidden").map(|v| v == "true").unwrap_or(false);

    if search_query.is_empty() {
        return Err(AppError::bad_request("Invalid search query"));
    }

    let full_path = resolve(&state, &internal_id, &search_path)?;
    let root = server_root(&state, &internal_id);
    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, &token)
        .await
        .cargo_files;
    let needle = search_query.to_lowercase();

    let mut results: Vec<serde_json::Value> = Vec::new();
    for entry in WalkDir::new(&full_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .take(20_000)
    {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_full = entry
            .path()
            .strip_prefix(&root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();

        let cargo_match = cargo_files
            .iter()
            .find(|c| c.target_path.trim_start_matches('/') == rel_full.trim_start_matches('/'));

        if cargo_match.map(|c| c.properties.is_hidden()).unwrap_or(false) && !show_hidden {
            continue;
        }

        if name.to_lowercase().contains(&needle) {
            if let Ok(meta) = entry.metadata() {
                let display_name = entry
                    .path()
                    .strip_prefix(&full_path)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string();
                results.push(file_stats_json(
                    &display_name,
                    &meta,
                    cargo_match.is_some(),
                    cargo_match.map(|c| &c.properties),
                ));
            }
        }

        if results.len() >= 500 {
            break;
        }
    }

    Ok(Json(json!({ "results": results })).into_response())
}

pub async fn compress(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path(_server_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let files: Vec<String> = body
        .get("files")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let destination = body
        .get("destination")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if files.is_empty() {
        return Err(AppError::bad_request("No files specified"));
    }

    let destination_path = resolve(&state, &internal_id, &destination)?;

    let (is_cargo, props) = cargo_properties(&state, &internal_id, &token, &destination).await;
    if is_cargo && props.as_ref().map(|p| p.is_readonly()).unwrap_or(false) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Cannot overwrite readonly cargo file", "cargoFile": true })),
        )
            .into_response());
    }

    // Stage all sources into a temp dir tree mirroring basenames, then zip.
    let staging = std::env::temp_dir().join(format!("blinkd-compress-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&staging).await?;

    let zip_result: AppResult<()> = async {
        for file in &files {
            let src = resolve(&state, &internal_id, file)?;
            let target = staging.join(StdPath::new(file).file_name().unwrap_or_default());
            if tokio::fs::metadata(&src).await?.is_dir() {
                copy_dir_recursive(&src, &target).await?;
            } else {
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(&src, &target).await?;
            }
        }
        create_zip(&staging, &destination_path, "archive")?;
        Ok(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&staging);
    zip_result?;

    Ok(Json(json!({ "success": true })).into_response())
}

pub async fn cargo_info(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    Path((_server_id, path)): Path<(String, String)>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, &token)
        .await
        .cargo_files;
    let normalized = path.trim_start_matches('/');
    let cargo = cargo_files
        .iter()
        .find(|c| c.target_path.trim_start_matches('/') == normalized);

    match cargo {
        Some(c) => Ok(Json(json!({
            "isCargoFile": true,
            "properties": {
                "hidden": c.properties.is_hidden(),
                "readonly": c.properties.is_readonly(),
                "noDelete": c.properties.is_no_delete(),
                "customProperties": c.properties.custom_properties.clone().unwrap_or(json!({}))
            }
        }))
        .into_response()),
        None => Err(AppError::not_found("Not a cargo file")),
    }
}

pub async fn cargo_files(
    State(state): State<Arc<AppState>>,
    Extension(validation): Extension<ValidateResponse>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Path(_server_id): Path<String>,
) -> AppResult<Response> {
    let internal_id = validation
        .server
        .as_ref()
        .map(|s| s.internal_id.clone())
        .unwrap_or_default();
    let token = extract_bearer(&headers);

    let query = query_map(&raw_query);
    let show_hidden = query.get("showHidden").map(|v| v == "true").unwrap_or(false);

    let cargo_files = state
        .panel
        .fetch_cargo_files(&internal_id, &token)
        .await
        .cargo_files;
    let filtered: Vec<&CargoFile> = if show_hidden {
        cargo_files.iter().collect()
    } else {
        cargo_files.iter().filter(|c| !c.properties.is_hidden()).collect()
    };

    Ok(Json(json!({ "cargoFiles": filtered })).into_response())
}
