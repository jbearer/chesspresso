use alloy::{primitives::Bytes, sol_types::SolEvent};
use anyhow::{bail, ensure, Context};
use chesspresso_core::{
    db::Db,
    game::{Game, Outcome},
    message::{Advance, Metadata, Report, Status},
    notice::{self},
};
use futures::stream::TryStreamExt;
use hyper::{client::connect::HttpConnector, Body, Response, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use std::env;
use tracing_subscriber::filter::EnvFilter;

struct App {
    db: Db,
    client: hyper::Client<HttpConnector>,
    server_addr: String,
}

impl App {
    async fn handle_advance(&mut self, mut request: Value) -> anyhow::Result<()> {
        let data = request["data"]
            .as_object_mut()
            .context("invalid request: not an object")?;

        let meta = data
            .remove("metadata")
            .context("invalid request: missing metadata")?;
        let meta: Metadata = serde_json::from_value(meta)?;

        let payload = data
            .remove("payload")
            .context("invalid request: missing payload")?;
        let message = payload
            .as_str()
            .context("invalid_request: payload not a string")?;
        let message = message.strip_prefix("0x").unwrap_or(message);
        let bytes = hex::decode(message)?;

        match serde_json::from_slice(&bytes)? {
            Advance::Challenge {
                opponent,
                first_move,
            } => {
                tracing::info!(%opponent, ?first_move, "challenge");
                let (white, black) = if first_move.is_some() {
                    (meta.msg_sender, opponent)
                } else {
                    (opponent, meta.msg_sender)
                };

                let mut game = self.db.new_game(white, black).await?;
                if let Some(san) = first_move {
                    let m = game.play(
                        meta.msg_sender,
                        game.hash(),
                        san.parse().context("invalid first move")?,
                    )?;
                    self.db.record_move(game.id(), m).await?;
                }
            }
            Advance::Move { id, hash, san } => {
                tracing::info!(%id, san, "move");
                let mut game = self.db.game(id).await?;
                let m = game.play(meta.msg_sender, hash, san.parse().context("invalid move")?)?;
                self.db.record_move(id, m).await?;

                // Check for game over.
                if let Some(outcome) = game.outcome() {
                    self.end_game(&game, outcome).await?;
                }
            }
            Advance::Resign { id, hash } => {
                tracing::info!(%id, "resign");

                let game = self.db.game(id).await?;
                ensure!(
                    game.hash() == hash,
                    "game is not in the expected state to resign"
                );
                ensure!(game.outcome().is_none(), "game is already over");

                let color = game
                    .player_color(meta.msg_sender)
                    .context("player is not in this game")?;
                let opponent = game.player(!color);

                self.end_game(
                    &game,
                    Outcome::Resignation {
                        winner: opponent,
                        loser: meta.msg_sender,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn handle_inspect(&mut self, mut request: Value) -> anyhow::Result<()> {
        tracing::info!(?request, "inspect");
        let data = request["data"]
            .as_object_mut()
            .context("invalid request: not an object")?;

        let payload = data
            .remove("payload")
            .context("invalid request: missing payload")?;
        let message = payload
            .as_str()
            .context("invalid_request: payload not a string")?;
        let message = message.strip_prefix("0x").unwrap_or(message);
        let bytes = hex::decode(message)?;
        let path = std::str::from_utf8(&bytes)?;
        let mut segments = path.split('/');

        match segments.next().context("no request")? {
            "games" => {
                let address = segments
                    .next()
                    .context("missing parameter address")?
                    .parse()?;
                let after = segments.next().map(|after| after.parse()).transpose()?;
                let games = self.db.games(address, after).try_collect().await?;
                self.report(&Report::Games { games }).await?;
            }
            "moves" => {
                let id = segments
                    .next()
                    .context("missing parameter game ID")?
                    .parse()?;
                let from = segments.next().context("missing parameter from")?.parse()?;
                let moves = self.db.moves(id, from).try_collect().await?;
                self.report(&Report::Moves { moves }).await?;
            }
            "stats" => {
                let address = segments
                    .next()
                    .context("missing parameter address")?
                    .parse()?;
                let stats = self.db.user_stats(address).await?;
                self.report(&Report::UserStats { stats }).await?;
            }
            req => {
                bail!("unsupported inspect request {req}");
            }
        }

        Ok(())
    }

    async fn end_game(&mut self, game: &Game, outcome: Outcome) -> anyhow::Result<()> {
        let notation = self.db.game_notation(game.id()).await?;

        if let Some((winner, loser)) = outcome.winner_loser() {
            self.notice(&notice::Victory {
                id: game.id().into(),
                winner,
                loser,
                message: outcome.to_string(),
                notation,
            })
            .await?;
        } else {
            self.report(&Report::Draw {
                id: game.id(),
                message: outcome.to_string(),
                notation,
            })
            .await?;
        }

        self.db.end_game(game, Some(outcome)).await?;
        Ok(())
    }

    async fn notice<T: SolEvent>(&self, payload: &T) -> anyhow::Result<()> {
        let mut data = T::SIGNATURE_HASH.0.to_vec();
        data.extend(Vec::from(payload.encode_log_data().data));

        let response = self
            .post("notice", json!({"payload": Bytes::from(data)}))
            .await?;
        ensure!(
            response.status().is_success(),
            "failed to post notice: {}",
            response.status()
        );
        Ok(())
    }

    async fn report(&self, payload: &Report) -> anyhow::Result<()> {
        let data = serde_json::to_string(payload)?;
        let response = self
            .post(
                "report",
                json!({"payload": Bytes::from(data.as_bytes().to_vec())}),
            )
            .await?;
        ensure!(
            response.status().is_success(),
            "failed to post report: {}",
            response.status()
        );
        Ok(())
    }

    async fn post(&self, endpoint: &str, body: impl Serialize) -> anyhow::Result<Response<Body>> {
        let request = hyper::Request::builder()
            .method(hyper::Method::POST)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .uri(format!("{}/{endpoint}", &self.server_addr))
            .body(hyper::Body::from(serde_json::to_string(&body)?))?;
        let response = self.client.request(request).await?;
        Ok(response)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_ansi(true)
        .init();

    let mut app = App {
        db: Db::memory().await?,
        client: hyper::Client::new(),
        server_addr: env::var("ROLLUP_HTTP_SERVER_URL")?,
    };

    let mut status = Status::Accept;
    loop {
        tracing::debug!("Sending finish");
        let response = app.post("finish", json!({"status": status})).await?;
        tracing::info!("Received finish status {}", response.status());

        if response.status() == StatusCode::ACCEPTED {
            tracing::info!("No pending rollup request, trying again");
        } else {
            let body = hyper::body::to_bytes(response).await?;
            let req: Value = serde_json::from_slice(&body)
                .context(format!("invalid finish response: {body:?}"))?;

            let request_type = req["request_type"]
                .as_str()
                .ok_or("request_type is not a string")?;
            status = match request_type {
                "advance_state" => match app.handle_advance(req).await {
                    Ok(()) => Status::Accept,
                    Err(err) => {
                        tracing::error!("{err:#}");
                        Status::Reject
                    }
                },
                "inspect_state" => match app.handle_inspect(req).await {
                    Ok(()) => Status::Accept,
                    Err(err) => {
                        tracing::error!("{err:#}");
                        Status::Reject
                    }
                },
                &_ => {
                    tracing::warn!("Unknown request type");
                    Status::Reject
                }
            };
        }
    }
}
