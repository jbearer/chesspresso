use crate::game::{GameHash, GameId};
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Advance {
    /// Challenge an opponent to a game.
    ///
    /// If provided, `first_move` (in SAN notation) will be executed immediately, and the challenger
    /// plays as white. Otherwise, the challenger plays as black, and it is up to the opponent to
    /// make the first move (implicitly accepting the challenge).
    ///
    /// Once created, a challenge manifests as a notice posted to the base layer listing the players
    /// and game ID.
    Challenge {
        opponent: Address,
        first_move: Option<String>,
    },
    /// Make a move in an existing game.
    Move {
        id: GameId,
        hash: GameHash,
        san: String,
    },
    /// Resign a game.
    Resign { id: GameId, hash: GameHash },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Report {
    /// Notification that a game ended in a draw.
    Draw {
        id: GameId,
        message: String,
        notation: String,
    },

    /// Response to /inspect/games
    Games { games: Vec<Game> },

    /// Response to /inspect/moves
    Moves { moves: Vec<String> },

    /// Response to /inspect/stats
    UserStats { stats: UserStats },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Game {
    pub id: GameId,
    pub white: Address,
    pub black: Address,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserStats {
    pub elo: f64,

    pub white_wins: u16,
    pub white_losses: u16,
    pub white_draws: u16,

    pub black_wins: u16,
    pub black_losses: u16,
    pub black_draws: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub block_number: u64,
    pub epoch_index: u64,
    pub input_index: u64,
    pub msg_sender: Address,
    pub timestamp: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Accept,
    Reject,
}
