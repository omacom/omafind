//! The socket: one thread per connection, JSON lines.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use anyhow::{Context, Result, bail};
use dig_client::{Event, Request, Response};
use dig_store::Store;

use crate::daemon::{Msg, Shared, status};
use crate::searcher::{Query, Searcher};

pub fn start(
    shared: Arc<Shared>,
    searcher: Arc<Searcher>,
    tx: Sender<Msg>,
    index_path: PathBuf,
) -> Result<()> {
    let path = dig_client::socket_path();
    if UnixStream::connect(&path).is_ok() {
        bail!("another digd is already listening on {}", path.display());
    }
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::thread::Builder::new()
        .name("dig-server".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let shared = shared.clone();
                let searcher = searcher.clone();
                let tx = tx.clone();
                let index_path = index_path.clone();
                std::thread::spawn(move || {
                    if let Err(error) = serve(stream, shared, searcher, tx, index_path) {
                        crate::log!("connection: {error:#}");
                    }
                });
            }
        })?;
    Ok(())
}

fn serve(
    stream: UnixStream,
    shared: Arc<Shared>,
    searcher: Arc<Searcher>,
    tx: Sender<Msg>,
    index_path: PathBuf,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                reply(
                    &mut writer,
                    &Response::Error {
                        message: format!("bad request: {error}"),
                    },
                )?;
                continue;
            }
        };
        match request {
            Request::Subscribe => {
                let (event_tx, event_rx) = channel::<Event>();
                shared.subscribers.lock().unwrap().push(event_tx);
                // The current state first, so a new subscriber never starts blank.
                let store = Store::open(&index_path)?;
                let first = Event::Status(status(&shared, &store, &searcher)?);
                send_event(&mut writer, &first)?;
                for event in event_rx {
                    if send_event(&mut writer, &event).is_err() {
                        return Ok(());
                    }
                }
                return Ok(());
            }
            other => {
                let response =
                    handle(other, &shared, &searcher, &tx, &index_path).unwrap_or_else(|error| {
                        Response::Error {
                            message: format!("{error:#}"),
                        }
                    });
                reply(&mut writer, &response)?;
            }
        }
    }
}

fn handle(
    request: Request,
    shared: &Shared,
    searcher: &Searcher,
    tx: &Sender<Msg>,
    index_path: &Path,
) -> Result<Response> {
    Ok(match request {
        Request::Search {
            query,
            limit,
            kinds,
            quick,
            session,
        } => {
            let reply = searcher.search(Query {
                text: &query,
                limit: limit.unwrap_or(20),
                kinds,
                quick,
                session: session.as_deref(),
            })?;
            hits_response(reply)
        }
        Request::Similar {
            chunk_id,
            limit,
            code,
        } => hits_response(searcher.similar(chunk_id, limit.unwrap_or(40), code)?),
        Request::Opened { path, query: _ } => {
            searcher.opened(&path)?;
            Response::Ok
        }
        Request::Recents => Response::Recents(searcher.recents()?),
        Request::Context { chunk_id, code } => Response::Passage(searcher.context(chunk_id, code)?),
        Request::Status => {
            let store = Store::open(index_path)?;
            Response::Status(status(shared, &store, searcher)?)
        }
        Request::Pause => {
            tx.send(Msg::Pause)?;
            Response::Ok
        }
        Request::Resume => {
            tx.send(Msg::Resume)?;
            Response::Ok
        }
        Request::Reindex => {
            tx.send(Msg::Crawl)?;
            Response::Ok
        }
        Request::Warm => {
            searcher.warm()?;
            Response::Ok
        }
        Request::Idle => {
            searcher.idle();
            Response::Ok
        }
        Request::SetRoots { folders, code } => {
            tx.send(Msg::SetRoots { folders, code })?;
            Response::Ok
        }
        Request::Shutdown => {
            tx.send(Msg::Shutdown)?;
            Response::Ok
        }
        Request::Subscribe => unreachable!(),
    })
}

fn hits_response(reply: dig_client::Reply) -> Response {
    if reply.superseded {
        return Response::Superseded;
    }
    Response::Hits {
        hits: reply.hits,
        took_ms: reply.took_ms,
        counts: reply.counts,
        chips: reply.chips,
        quick: reply.quick,
    }
}

fn reply(writer: &mut UnixStream, response: &Response) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    Ok(())
}

fn send_event(writer: &mut UnixStream, event: &Event) -> Result<()> {
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    Ok(())
}
