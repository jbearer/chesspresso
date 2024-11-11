use crate::Indexer;
use alloy::primitives::Address;
use anyhow::{bail, ensure, Context};
use chesspresso_core::{
    game::{GameId, San},
    message::{Game, Report, UserStats},
};
use futures::stream::{self, Stream, StreamExt};
use hyper::{client::connect::HttpConnector, Client, Method, Request};
use serde_json::{Map, Value};
use std::time::Duration;
use tokio::time::sleep;
use url::Url;

#[derive(Clone, Debug)]
pub struct InspectIndexer {
    client: Client<HttpConnector>,
    node_url: Url,
    polling_interval: Duration,
}

impl InspectIndexer {
    pub fn new(node_url: Url) -> Self {
        Self {
            client: Client::new(),
            node_url,
            polling_interval: Duration::from_secs(2),
        }
    }

    async fn inspect(&self, endpoint: &str) -> anyhow::Result<Report> {
        let url = format!("{}inspect/{endpoint}", &self.node_url);
        let request = Request::builder()
            .method(Method::GET)
            .uri(&url)
            .body(Default::default())?;
        let response = self.client.request(request).await?;
        ensure!(
            response.status().is_success(),
            "{url}: inspect error: {}",
            response.status()
        );

        let body = hyper::body::to_bytes(response).await?;
        let mut inspect: Map<String, Value> = serde_json::from_slice(&body)?;

        let mut reports = inspect.remove("reports").context("missing reports")?;
        let reports = reports.as_array_mut().context("malformed reports")?;
        ensure!(
            reports.len() == 1,
            "reports is not a singleton array: {reports:?}"
        );

        let mut report = reports.remove(0);
        let report = report.as_object_mut().context("report is not an object")?;

        let payload = report.remove("payload").context("malformed report")?;
        let payload = payload.as_str().context("malformed report payload")?;
        let payload = payload.strip_prefix("0x").unwrap_or(payload);

        let bytes = hex::decode(payload)?;
        let report = serde_json::from_slice(&bytes)?;
        Ok(report)
    }
}

impl Indexer for InspectIndexer {
    fn games_with_user(
        &self,
        address: Address,
        after: Option<GameId>,
    ) -> impl Stream<Item = Game> + Unpin {
        stream::unfold((self.clone(), after), move |(indexer, after)| async move {
            sleep(indexer.polling_interval).await;

            let mut request = format!("games/{address}");
            if let Some(after) = after {
                request = format!("{request}/{after}");
            }
            let games = match indexer.inspect(&request).await {
                Ok(Report::Games { games }) => games,
                Ok(report) => {
                    tracing::warn!(?report, "unexpected report, expected games");
                    return Some((stream::iter(vec![]), (indexer, after)));
                }
                Err(err) => {
                    tracing::warn!("error in games stream: {err:#}");
                    return Some((stream::iter(vec![]), (indexer, after)));
                }
            };
            let after = games.last().map(|game| Some(game.id)).unwrap_or(after);

            Some((stream::iter(games), (indexer, after)))
        })
        .flatten()
        .boxed()
    }

    fn moves(&self, id: GameId, from: u16) -> impl Stream<Item = San> + Unpin {
        stream::unfold((self.clone(), from), move |(indexer, from)| async move {
            sleep(indexer.polling_interval).await;

            let moves = match indexer.inspect(&format!("moves/{id}/{from}")).await {
                Ok(Report::Moves { moves }) => moves,
                Ok(report) => {
                    tracing::warn!(?report, "unexpected report, expected moves");
                    return Some((stream::iter(vec![]), (indexer, from)));
                }
                Err(err) => {
                    tracing::warn!("error in moves stream: {err:#}");
                    return Some((stream::iter(vec![]), (indexer, from)));
                }
            };
            let moves: Vec<San> = match moves.into_iter().map(|san| san.parse()).collect() {
                Ok(moves) => moves,
                Err(err) => {
                    tracing::warn!("error parsing moves: {err:#}");
                    return Some((stream::iter(vec![]), (indexer, from)));
                }
            };
            let from = from + (moves.len() as u16);
            Some((stream::iter(moves), (indexer, from)))
        })
        .flatten()
        .boxed()
    }

    async fn user_stats(&self, address: Address) -> anyhow::Result<UserStats> {
        match self.inspect(&format!("stats/{address}")).await? {
            Report::UserStats { stats } => Ok(stats),
            report => bail!("unexpected report, expected user stats: {report:?}"),
        }
    }
}
