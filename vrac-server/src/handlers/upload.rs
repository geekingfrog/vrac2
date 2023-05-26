use async_zip::base::write::owned_writer;
use async_zip::error::ZipError;
use async_zip::{Compression, ZipEntry, ZipEntryBuilder};
use futures::io::BufReader;
use futures::{AsyncWrite as FAsyncWrite, Future, FutureExt};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::mem;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};

use axum::extract::{Multipart, Path, Query};
use axum::response::{Redirect, Response};
use axum::{extract::State, response::Html, response::IntoResponse};
use axum_flash::IncomingFlashes;
use humantime::format_duration;
use serde::{de, Deserialize};
use time::{Duration, OffsetDateTime};

use futures::TryStreamExt;
use tokio::io::{AsyncWrite, DuplexStream};
use tokio_util::compat::{
    Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt,
};

use pin_project::pin_project;

use crate::db::{DbFile, DbToken, GetTokenResult};
use crate::error::Result;
use crate::handlers::flash_utils::ctx_from_flashes;
use crate::state::AppState;
use crate::upload::{InitFile, StorageBackend};

type StdResult<T, E> = std::result::Result<T, E>;

// wrapper because I later need a futures::AsyncWrite, but tokio's File implements
// tokio::io::AsyncWrite so this bridges the two.
#[pin_project]
struct FutureFile {
    #[pin]
    inner: tokio::fs::File,
}

impl futures::AsyncWrite for FutureFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

/// How to render a File in a template from a DB file
#[derive(serde::Serialize, Debug)]
struct TplFile {
    id: i64,
    mime_type: Option<String>,
    mime_prefix: Option<String>,
    name: Option<String>,
}

impl std::convert::From<DbFile> for TplFile {
    fn from(f: DbFile) -> Self {
        Self {
            id: f.id,
            mime_type: f.mime_type.clone(),
            mime_prefix: f.mime_type.and_then(|m| match m.split_once('/') {
                Some((x, _)) => Some(x.to_string()),
                None => None,
            }),
            name: f.name,
        }
    }
}

pub(crate) async fn get_upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    Path(tok_path): Path<String>,
    Query(file_query): Query<FileQuery>,
) -> Result<Response> {
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::NotFound => {
            let html: Html<String> = state
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?
                .into();
            let rsp = (hyper::StatusCode::NOT_FOUND, html);
            Ok((incoming_flashes, rsp).into_response())
        }
        GetTokenResult::Fresh(tok) => upload_form(state, incoming_flashes, tok).await,
        GetTokenResult::Used(tok) => {
            if file_query.zip {
                get_files_zip(state, incoming_flashes, tok).await
            } else {
                get_files_html(state, incoming_flashes, tok).await
            }
        }
    }
}

pub(crate) async fn post_upload_form(
    Path(tok_path): Path<String>,
    state: State<AppState>,
    mut multipart: Multipart,
) -> Result<Response> {
    // TODO: maybe make a custom extractor for the token which handles the
    // urldecoding itself to reduce duplication?
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    let token = match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::Fresh(t) => t,
        GetTokenResult::NotFound | GetTokenResult::Used(_) => {
            let not_found = state
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?;
            return Ok(not_found.into_response());
        }
    };
    let token = state.db.initiate_upload(token).await?;

    let mut total_bytes = 0;
    let mut file_idx = 0;
    while let Some(field) = multipart.next_field().await? {
        file_idx += 1;
        tracing::info!(
            "got a new field here {:?} of type {:?} for file {:?}",
            field.name(),
            field.content_type(),
            field.file_name(),
        );

        let mime_type = field.content_type();
        tracing::info!("mime type: {mime_type:?}");
        let init_file = InitFile {
            token_id: token.id,
            token_path: &token.path,
            file_index: file_idx,
            attempt_counter: token.attempt_counter,
            mime_type,
            file_name: field.file_name(),
        };

        let (writer, data) = state.storage_fs.initiate_upload(&init_file).await?;
        let mut writer = writer.compat_write();
        let db_file = state
            .db
            .create_file(
                &token,
                state.storage_fs.get_type(),
                serde_json::to_string(&data)?,
                mime_type,
                field.file_name(),
            )
            .await?;

        let reader =
            field.map_err(|err| std::io::Error::new(ErrorKind::Other, format!("oops {err:?}")));
        let bytes_copied =
            futures::io::copy_buf(&mut reader.into_async_read(), &mut writer).await?;
        total_bytes += bytes_copied;

        if bytes_copied == 0 {
            tracing::info!("No bytes uploaded for token {} - {}", token.id, token.path);
            state.storage_fs.delete_blob(data).await?;
            state.db.delete_files([db_file.id]).await?;
        } else {
            let mb_data = state
                .storage_fs
                .finalize_upload(writer.into_inner())
                .await?;
            state
                .db
                .finalise_file_upload(
                    db_file,
                    mb_data.map(|d| serde_json::to_string(&d)).transpose()?,
                )
                .await?;

            tracing::info!("total uploaded for field: {}Kib", bytes_copied / 1024);
        }
    }

    if total_bytes == 0 {
        tracing::info!(
            "No bytes uploaded at all for token {} - {}",
            token.id,
            token.path
        );
    } else {
        state.db.finalise_token_upload(token).await?;
        tracing::info!("done with upload");
    }

    // TODO: maybe use https://docs.rs/axum/0.6.0-rc.4/axum/extract/struct.OriginalUri.html
    // instead of reconstructing the path here
    Ok(Redirect::to(&format!("/f/{}", tok_path)).into_response())
}

async fn upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    tracing::info!("fresh token {} - {}", tok.id, tok.path);
    let now = OffsetDateTime::now_utc();
    let duration = tok.valid_until - now;
    let duration = std::time::Duration::from_secs(duration.as_seconds_f64().round() as u64);

    let mut ctx = ctx_from_flashes(&incoming_flashes);
    ctx.insert("max_size", &tok.max_size_mib);
    ctx.insert("valid_for", &format_duration(duration).to_string());
    if let Some(d) = tok.content_expires_after_hours {
        let d = std::time::Duration::new((d as u64) * 3600, 0);
        ctx.insert("content_duration", &format_duration(d).to_string());
    }

    let html: Html<String> = state
        .templates
        .read()
        .render("upload_form.html", &ctx)?
        .into();
    Ok((incoming_flashes, html).into_response())
}

async fn get_files_html(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    let mut ctx = tera::Context::new();
    ctx.insert(
        "expires_at",
        &tok.content_expires_at.map(|d| {
            let fmt = time::macros::format_description!("[year]/[month]/[day] [hour]:[minute]");
            d.format(&fmt).expect("formatting offsetdatetime")
        }),
    );

    ctx.insert(
        "expires_in",
        &tok.content_expires_at.map(|expires_at| {
            let now = OffsetDateTime::now_utc();
            let mut d = expires_at - now;
            let mut res = String::new();
            let days = d.whole_days();
            d = d - Duration::days(days);
            let hours = d.whole_hours();
            d = d - Duration::hours(hours);
            let minutes = d.whole_minutes();
            if days > 0 {
                res.push_str(&format!("{} days ", days));
            }
            if hours > 0 {
                res.push_str(&format!("{} hours ", hours));
            }
            if minutes > 0 {
                res.push_str(&format!("{} minutes ", minutes));
            }

            res
        }),
    );

    ctx.insert("token_path", &tok.path);

    let files = state.db.get_files(tok.id, tok.attempt_counter).await?;
    let files: Vec<TplFile> = files.into_iter().map(|f| f.into()).collect();

    ctx.insert("files", &files);

    let html: Html<String> = state
        .templates
        .read()
        .render("get_files.html", &ctx)?
        .into();
    Ok((incoming_flashes, html).into_response())
}

// can't be arsed to type that so many times
type InMemWrt = Compat<DuplexStream>;
struct ZipFiles {
    state: ZipWriterState,
    read_buf: Compat<DuplexStream>,
    entries: VecDeque<(ZipEntry, Box<dyn futures::io::AsyncBufRead + Unpin + Send>)>,
}

enum ZipWriterState {
    /// the zip writer is open and ready for new entries to be added
    Archive(owned_writer::ZipWriterArchive<InMemWrt>),
    /// in the process of adding a new entry to the archive (writing the headers)
    OpeningEntry {
        fut: Pin<
            Box<
                dyn Future<Output = StdResult<owned_writer::ZipWriterEntry<InMemWrt>, ZipError>>
                    + Send,
            >,
        >,
        rdr: Box<dyn futures::io::AsyncBufRead + Unpin + Send>,
    },
    /// in the process of copying the bytes from the reader into the zip archive
    CopyEntry {
        rdr: Pin<Box<dyn futures::io::AsyncBufRead + Unpin + Send>>,
        wrt: owned_writer::ZipWriterEntry<Compat<DuplexStream>>,
    },
    /// done with the entry, need to write the crc and other metadata after the entry
    ClosingEntry(
        Pin<
            Box<
                dyn Future<
                        Output = std::result::Result<
                            owned_writer::ZipWriterArchive<InMemWrt>,
                            ZipError,
                        >,
                    > + Send,
            >,
        >,
    ),
    /// done with all the entries, we can finalize the zip archive
    ClosingArchive(Pin<Box<dyn Future<Output = std::io::Result<usize>> + Send>>),
    /// the archive is closed, but we may still have bytes in our internal read
    /// buffer, so need to flush those
    Flushing,
    // this variant is merely here because I need a value for mem::replace
    // otherwise I run into issues when trying to assign a new state while
    // moving/consumming the old one.
    Dummy,
}

impl ZipFiles {
    fn new(entries: Vec<(ZipEntry, Box<dyn futures::io::AsyncBufRead + Unpin + Send>)>) -> Self {
        let (rdr, wrt) = tokio::io::duplex(4096);
        Self {
            state: ZipWriterState::Archive(owned_writer::ZipWriterArchive::new(wrt.compat_write())),
            read_buf: rdr.compat(),
            entries: entries.into(),
        }
    }
}

trait IntoIOError {
    // fn into_io_error<E: std::error::Error + Send + Sync + 'static>(self: E) -> std::io::Error;
    fn into_io_error(self) -> std::io::Error;
}

impl IntoIOError for ZipError {
    fn into_io_error(self) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Other, self)
    }
}

impl futures::io::AsyncRead for ZipFiles {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        // see if we already have some compressed bytes ready to go (or an error)
        if let Poll::Ready(x) = Pin::new(&mut self.read_buf).poll_read(cx, buf) {
            match x {
                Ok(n) if n == 0 => {
                    // if the archive is closed, and we EOF the internal read buffer
                    // that means we're completely done and we can return the EOF
                    if matches!(self.state, ZipWriterState::Flushing) {
                        return Poll::Ready(Ok(0));
                    }
                }
                x => {
                    return Poll::Ready(x);
                }
            }
        }

        // match &self.state {
        //     ZipWriterState::Archive(_) => tracing::debug!("in state archive"),
        //     ZipWriterState::OpeningEntry { .. } => tracing::debug!("in state opening entry"),
        //     ZipWriterState::CopyEntry { .. } => tracing::debug!("in state copy entry"),
        //     ZipWriterState::ClosingEntry(_) => tracing::debug!("in state closing entry"),
        //     ZipWriterState::ClosingArchive(_) => tracing::debug!("in state closing archive"),
        //     ZipWriterState::Flushing => tracing::debug!("in state flushing"),
        //     ZipWriterState::Dummy => tracing::debug!("in state dummy"),
        // }

        let state = mem::replace(&mut self.state, ZipWriterState::Dummy);

        // in every branch, we need to be careful of setting the state properly if we're intending
        // on looping (recursive call)
        match state {
            ZipWriterState::Archive(wrt) => {
                match self.entries.pop_front() {
                    Some((entry, rdr)) => {
                        tracing::debug!(
                            "moving onto the next entry {:?}",
                            entry.filename().as_str()
                        );
                        let writer_entry_fut = wrt.write_entry_stream(entry);
                        let writer_entry_fut = Box::pin(writer_entry_fut);
                        self.state = ZipWriterState::OpeningEntry {
                            fut: writer_entry_fut,
                            rdr,
                        };
                        Pin::new(&mut self).poll_read(cx, buf)
                    }
                    None => {
                        // there are no more entries to write, so we need to close
                        // the zip archive and we're done
                        tracing::debug!("no more entry, closing the archive");
                        let fut = wrt.close().map(|res| match res {
                            Ok(_) => Ok(0),
                            Err(err) => Err(err.into_io_error()),
                        });
                        self.state = ZipWriterState::ClosingArchive(Box::pin(fut));
                        Pin::new(&mut self).poll_read(cx, buf)
                    }
                }
            }
            ZipWriterState::OpeningEntry { mut fut, rdr } => {
                let wrt = match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(writer_entry)) => writer_entry,
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err.into_io_error())),
                    Poll::Pending => {
                        self.state = ZipWriterState::OpeningEntry { fut, rdr };
                        return Poll::Pending;
                    }
                };
                self.state = ZipWriterState::CopyEntry {
                    rdr: Pin::new(rdr),
                    wrt,
                };
                Pin::new(&mut self).poll_read(cx, buf)
            }
            ZipWriterState::CopyEntry { mut rdr, mut wrt } => {
                // this is copied from futures::io::copy_buf, but without the loop
                // since I want to extract the bytes from the duplexstream asap
                // and also some annoying state manipulation
                let buffer = match rdr.as_mut().poll_fill_buf(cx) {
                    Poll::Ready(x) => x?,
                    Poll::Pending => {
                        self.state = ZipWriterState::CopyEntry { rdr, wrt };
                        return Poll::Pending;
                    }
                };

                if buffer.is_empty() {
                    match Pin::new(&mut wrt).poll_flush(cx) {
                        Poll::Ready(x) => x?,
                        Poll::Pending => {
                            self.state = ZipWriterState::CopyEntry { rdr, wrt };
                            return Poll::Pending;
                        }
                    }
                    let fut = wrt.close();
                    self.state = ZipWriterState::ClosingEntry(Box::pin(fut));
                    return Pin::new(&mut self).poll_read(cx, buf);
                }

                let i = match Pin::new(&mut wrt).poll_write(cx, buffer) {
                    Poll::Ready(x) => x?,
                    Poll::Pending => {
                        self.state = ZipWriterState::CopyEntry { rdr, wrt };
                        return Poll::Pending;
                    }
                };

                if i == 0 {
                    return Poll::Ready(Err(std::io::ErrorKind::WriteZero.into()));
                }
                rdr.as_mut().consume(i);
                self.state = ZipWriterState::CopyEntry { rdr, wrt };
                Pin::new(&mut self).poll_read(cx, buf)
            }
            ZipWriterState::ClosingEntry(mut fut) => {
                let wrt = match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(wrt)) => wrt,
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err.into_io_error())),
                    Poll::Pending => {
                        self.state = ZipWriterState::ClosingEntry(fut);
                        return Poll::Pending;
                    }
                };
                self.state = ZipWriterState::Archive(wrt);
                Pin::new(&mut self).poll_read(cx, buf)
            }
            ZipWriterState::ClosingArchive(mut fut) => {
                tracing::debug!("closing the archive");
                match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(_)) => {
                        self.state = ZipWriterState::Flushing;
                        Pin::new(&mut self).poll_read(cx, buf)
                    }
                    x => {
                        self.state = ZipWriterState::ClosingArchive(fut);
                        x
                    }
                }
            }
            ZipWriterState::Flushing => {
                self.state = ZipWriterState::Flushing;
                Pin::new(&mut self).poll_read(cx, buf)
            }
            ZipWriterState::Dummy => unreachable!("dummy state should never be reached"),
        }
    }
}

async fn get_files_zip(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    let files = state.db.get_files(tok.id, tok.attempt_counter).await?;

    // let mut fut = state
    //     .storage_fs
    //     .clone()
    //     .read_blob(serde_json::from_str(&files[0].backend_data).unwrap());

    let entries = vec![
        (
            ZipEntryBuilder::new("dilbert.gif".into(), Compression::Deflate).build(),
            Box::new(BufReader::new(
                tokio::fs::File::open("testfiles/dilbert.gif")
                    .await?
                    .compat_write(),
            )) as _,
        ),
        (
            ZipEntryBuilder::new("Suprise [fqyjOc3EpT4].webm".into(), Compression::Deflate).build(),
            Box::new(BufReader::new(
                tokio::fs::File::open("testfiles/Suprise [fqyjOc3EpT4].webm")
                    .await?
                    .compat_write(),
            )) as _,
        ),
    ];
    let zip_files = ZipFiles::new(entries);

    let stream = tokio_util::io::ReaderStream::new(zip_files.compat());
    let body = axum::body::StreamBody::new(stream);
    Ok((incoming_flashes, body).into_response())
}

#[derive(serde::Deserialize, Debug, Default)]
pub(crate) struct FileQuery {
    #[serde(default, deserialize_with = "true_if_present")]
    zip: bool,
}

// if the field is present at all, treat it as true, and ignore any associated value
fn true_if_present<'de, D>(de: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(true),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom),
    }
}
