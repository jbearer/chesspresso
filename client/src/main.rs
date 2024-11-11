use alloy::primitives::Address;
use chesspresso_core::{
    db::Db,
    game::{Game, GameId},
};
use chesspresso_indexer::{Indexer, InspectIndexer};
use clap::Parser;
use futures::{future, stream::StreamExt};
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{spawn, sync::Mutex, time::sleep};
use tracing::instrument;
use tracing_subscriber::EnvFilter;
use url::Url;

/// Client daemon for Chesspresso.
#[derive(Parser)]
struct Options {
    #[clap(short, long, env = "CHESSPRESSO_ADDRESS")]
    address: Address,

    #[clap(short, long, env = "CHESSPRESSO_DB")]
    db: Option<PathBuf>,

    #[clap(short = 'u', long, env = "CHESSPRESSO_NODE_URL")]
    node_url: Url,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_ansi(true)
        .init();
    let opt = Options::parse();

    let db_path = match opt.db {
        Some(path) => path,
        None => Path::new(&env::var("HOME")?).join(format!(".chesspresso/{}.sqlite", opt.address)),
    };
    let db = Arc::new(Mutex::new(Db::open(&db_path).await?));

    let indexer = InspectIndexer::new(opt.node_url);

    // Listen for new moves in the games we already have.
    {
        let mut conn = db.lock().await;
        let mut games = conn.games(opt.address, None);
        while let Some(game) = games.next().await {
            spawn(listen_moves(indexer.clone(), db.clone(), game?.id));
        }
    }

    // Listen for new games.
    spawn(listen_games(indexer.clone(), db.clone(), opt.address));

    // Block until killed.
    future::pending().await
}

#[instrument(skip(indexer, db))]
async fn listen_moves(indexer: impl Indexer, db: Arc<Mutex<Db>>, id: GameId) {
    let mut game = loop {
        match db.lock().await.game(id).await {
            Ok(game) => break game,
            Err(err) => {
                tracing::warn!("error loading game: {err:#}");
                sleep(Duration::from_secs(5)).await;
            }
        }
    };

    let mut moves = indexer.moves(id, game.half_move() + 1);
    while let Some(san) = moves.next().await {
        tracing::info!(%san, "new move");

        let m = match game.play_next_move(san.clone()) {
            Ok(m) => m,
            Err(err) => {
                tracing::error!(%san, "game reached invalid state: {err:#}");
                return;
            }
        };

        loop {
            let mut db = db.lock().await;
            let Err(err) = db.record_move(id, m.clone()).await else {
                break;
            };

            tracing::warn!(?m, "error saving move: {err:#}");
            sleep(Duration::from_secs(5)).await;
        }
    }

    tracing::info!("game over");
}

#[instrument(skip(indexer, db))]
async fn listen_games(
    indexer: impl Indexer + Clone + Send + 'static,
    db: Arc<Mutex<Db>>,
    address: Address,
) {
    let after = loop {
        match db.lock().await.max_game().await {
            Ok(id) => break id,
            Err(err) => {
                tracing::warn!("error loading max game: {err:#}");
                sleep(Duration::from_secs(5)).await;
            }
        }
    };

    let mut games = indexer.games_with_user(address, after);
    while let Some(game) = games.next().await {
        tracing::info!(?game, "new game");
        let id = loop {
            if let Err(err) = db
                .lock()
                .await
                .insert_game(&Game::new(game.id, game.white, game.black))
                .await
            {
                tracing::warn!(?game, "error saving challenge: {err:#}");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
            break game.id;
        };
        spawn(listen_moves(indexer.clone(), db.clone(), id));
    }

    tracing::info!("no more challenges");
}
