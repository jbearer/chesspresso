use crate::{
    game::{Game, GameId, Move, Outcome, San},
    message::{self, UserStats},
    rating,
};
use alloy::primitives::Address;
use anyhow::Context;
use derive_more::Into;
use futures::stream::{Stream, StreamExt, TryStreamExt};
use glicko2::{GameResult, Glicko2Rating, GlickoRating};
use sqlx::{
    migrate, query, query_as,
    sqlite::{Sqlite, SqliteConnectOptions, SqliteConnection},
    ConnectOptions, Connection, Transaction,
};
use std::path::Path;

#[derive(Debug)]
pub struct Db {
    conn: SqliteConnection,
}

impl Db {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        Self::new(
            SqliteConnectOptions::default()
                .filename(path)
                .create_if_missing(true),
        )
        .await
    }

    pub async fn memory() -> anyhow::Result<Self> {
        Self::new(Default::default()).await
    }

    async fn new(opt: SqliteConnectOptions) -> anyhow::Result<Self> {
        let mut conn = opt.connect().await?;
        migrate!("db/migrations").run(&mut conn).await?;
        Ok(Self { conn })
    }

    pub async fn new_game(&mut self, white: Address, black: Address) -> anyhow::Result<Game> {
        let mut tx = self.conn.begin().await?;

        // Ensure users exist.
        let unrated = rating::unrated();
        for address in [white, black] {
            query("INSERT OR IGNORE INTO user (address, elo_value, elo_deviation, elo_volatility) VALUES ($1, $2, $3, $4)")
                .bind(address.to_string())
                .bind(unrated.value)
                .bind(unrated.deviation)
                .bind(unrated.volatility)
                .execute(tx.as_mut())
                .await?;
        }

        let (id,): (i32,) =
            query_as("INSERT INTO game (white, black) VALUES ($1, $2) RETURNING id")
                .bind(white.to_string())
                .bind(black.to_string())
                .fetch_one(tx.as_mut())
                .await?;
        tx.commit().await?;

        tracing::debug!(id, %white, %black, "created new game");
        Ok(Game::new(id.into(), white, black))
    }

    pub async fn insert_game(&mut self, game: &Game) -> anyhow::Result<()> {
        query("INSERt INTO game (id, white, black) VALUES ($1, $2, $3)")
            .bind(i32::from(game.id()))
            .bind(game.white().to_string())
            .bind(game.black().to_string())
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn game(&mut self, id: GameId) -> anyhow::Result<Game> {
        let (white, black): (String, String) =
            query_as("SELECT white, black FROM game WHERE id = $1 LIMIT 1")
                .bind(i32::from(id))
                .fetch_optional(&mut self.conn)
                .await?
                .context(format!("game {id} not found"))?;
        let moves =
            query_as::<_, (String,)>("SELECT san FROM move WHERE game = $1 ORDER BY half_move")
                .bind(i32::from(id))
                .fetch(&mut self.conn)
                .map(|res| {
                    let (san,) = res?;
                    Ok::<San, anyhow::Error>(san.parse()?)
                })
                .try_collect::<Vec<_>>()
                .await?;
        Game::from_moves(id, white.parse()?, black.parse()?, moves)
    }

    pub async fn game_notation(&mut self, id: GameId) -> anyhow::Result<String> {
        let mut moves =
            query_as::<_, (String,)>("SELECT san FROM move WHERE game = $1 ORDER BY half_move")
                .bind(i32::from(id))
                .fetch_all(&mut self.conn)
                .await?
                .into_iter();

        let mut i = 1;
        let mut notation = String::new();
        while let Some((white_move,)) = moves.next() {
            notation = format!("{notation}{i}.{white_move} ");
            if let Some((black_move,)) = moves.next() {
                notation = format!("{notation}{black_move} ");
            } else {
                break;
            }
            i += 1;
        }
        Ok(notation)
    }

    pub async fn record_move(&mut self, id: GameId, m: Move) -> anyhow::Result<()> {
        query("INSERT INTO move (game, half_move, san) VALUES ($1, $2, $3)")
            .bind(i32::from(id))
            .bind(m.half_move() as i32)
            .bind(m.san())
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn end_game(&mut self, game: &Game, outcome: Option<Outcome>) -> anyhow::Result<()> {
        let mut tx = self.conn.begin().await?;

        if let Some(outcome) = outcome {
            if let Some((winner, loser)) = outcome.winner_loser() {
                let winner_current_elo = get_elo(&mut tx, winner).await?;
                let loser_current_elo = get_elo(&mut tx, loser).await?;

                set_elo(
                    &mut tx,
                    winner,
                    rating::update(winner_current_elo, GameResult::win(loser_current_elo)),
                )
                .await?;
                set_elo(
                    &mut tx,
                    loser,
                    rating::update(loser_current_elo, GameResult::loss(winner_current_elo)),
                )
                .await?;

                if winner == game.white() {
                    query("UPDATE user SET white_wins = white_wins + 1 WHERE address = $1")
                        .bind(winner.to_string())
                        .execute(tx.as_mut())
                        .await?;
                    query("UPDATE user SET black_losses = black_losses + 1 WHERE address = $1")
                        .bind(loser.to_string())
                        .execute(tx.as_mut())
                        .await?;
                } else {
                    query("UPDATE user SET black_wins = black_wins + 1 WHERE address = $1")
                        .bind(winner.to_string())
                        .execute(tx.as_mut())
                        .await?;
                    query("UPDATE user SET white_losses = white_losses + 1 WHERE address = $1")
                        .bind(loser.to_string())
                        .execute(tx.as_mut())
                        .await?;
                }
            } else {
                let white = game.white();
                let black = game.black();

                let white_current_elo = get_elo(&mut tx, white).await?;
                let black_current_elo = get_elo(&mut tx, black).await?;

                set_elo(
                    &mut tx,
                    white,
                    rating::update(white_current_elo, GameResult::draw(black_current_elo)),
                )
                .await?;
                set_elo(
                    &mut tx,
                    black,
                    rating::update(black_current_elo, GameResult::draw(white_current_elo)),
                )
                .await?;

                query("UPDATE user SET white_draws = white_draws + 1 WHERE address = $1")
                    .bind(white.to_string())
                    .execute(tx.as_mut())
                    .await?;
                query("UPDATE user SET black_draws = black_draws + 1 WHERE address = $1")
                    .bind(black.to_string())
                    .execute(tx.as_mut())
                    .await?;
            }
        }

        query("DELETE FROM game WHERE id = $1")
            .bind(i32::from(game.id()))
            .execute(tx.as_mut())
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub fn games(
        &mut self,
        address: Address,
        after: Option<GameId>,
    ) -> impl '_ + Stream<Item = anyhow::Result<message::Game>> {
        let from = after.map(|id| i32::from(id) + 1).unwrap_or_default();
        query_as(
            "SELECT id, white, black FROM game WHERE id >= $1 AND $2 IN (white, black) ORDER BY id",
        )
        .bind(from)
        .bind(address.to_string())
        .fetch(&mut self.conn)
        .map(|res| {
            let (id, white, black): (i32, String, String) = res?;
            Ok(message::Game {
                id: id.into(),
                white: white.parse()?,
                black: black.parse()?,
            })
        })
    }

    pub fn moves(
        &mut self,
        id: GameId,
        from: u16,
    ) -> impl '_ + Stream<Item = anyhow::Result<String>> {
        query_as("SELECT san FROM move WHERE game = $1 AND half_move >= $2 ORDER BY half_move")
            .bind(i32::from(id))
            .bind(from)
            .fetch(&mut self.conn)
            .map(|res| {
                let (m,) = res?;
                Ok(m)
            })
    }

    pub async fn max_game(&mut self) -> anyhow::Result<Option<GameId>> {
        let (Some(id),): (Option<i32>,) = query_as("SELECT max(id) FROM game")
            .fetch_one(&mut self.conn)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(id.into()))
    }

    pub async fn user_stats(&mut self, address: Address) -> anyhow::Result<UserStats> {
        let query = "
            SELECT
                elo_value,
                elo_deviation,
                elo_volatility,
                white_wins,
                white_losses,
                white_draws,
                black_wins,
                black_losses,
                black_draws
            FROM user WHERE address = $1 LIMIT 1";
        let (
            elo_value,
            elo_deviation,
            elo_volatility,
            white_wins,
            white_losses,
            white_draws,
            black_wins,
            black_losses,
            black_draws,
        ): (f64, f64, f64, i32, i32, i32, i32, i32, i32) = query_as(query)
            .bind(address.to_string())
            .fetch_optional(&mut self.conn)
            .await?
            .context(format!("unknown user {address}"))?;

        let elo = GlickoRating::from(Glicko2Rating {
            value: elo_value,
            deviation: elo_deviation,
            volatility: elo_volatility,
        });

        Ok(UserStats {
            elo: elo.value,
            white_wins: white_wins as u16,
            white_losses: white_losses as u16,
            white_draws: white_draws as u16,
            black_wins: black_wins as u16,
            black_losses: black_losses as u16,
            black_draws: black_draws as u16,
        })
    }
}

async fn get_elo<'c>(
    tx: &mut Transaction<'c, Sqlite>,
    address: Address,
) -> anyhow::Result<Glicko2Rating> {
    let (value, deviation, volatility) = query_as(
        "SELECT elo_value, elo_deviation, elo_volatility FROM user WHERE address = $1 LIMIT 1",
    )
    .bind(address.to_string())
    .fetch_one(tx.as_mut())
    .await?;
    Ok(Glicko2Rating {
        value,
        deviation,
        volatility,
    })
}

async fn set_elo<'c>(
    tx: &mut Transaction<'c, Sqlite>,
    address: Address,
    elo: Glicko2Rating,
) -> anyhow::Result<()> {
    query("UPDATE user SET (elo_value, elo_deviation, elo_volatility) = ($1, $2, $3) WHERE address = $4")
        .bind(elo.value)
        .bind(elo.deviation)
        .bind(elo.volatility)
        .bind(address.to_string())
        .execute(tx.as_mut())
        .await?;
    Ok(())
}
