use alloy::primitives::{keccak256, Address, FixedBytes};
use ansi_term::Style;
use anyhow::{ensure, Context};
use derive_more::{AsRef, Display, From, FromStr, Into};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use shakmaty::{
    san::{SanPlus, Suffix},
    Chess, File, Position, Rank, Square,
};

pub use shakmaty::{san::San, Color};

#[derive(
    Clone, Copy, Deserialize, Serialize, Debug, Display, From, FromStr, Into, PartialEq, Eq,
)]
#[display("{_0}")]
#[serde(transparent)]
pub struct GameId(i32);

/// A succinct representation of a game state.
///
/// A [`GameHash`] is a chained cryptographic hash starting from the initial game state (game ID and
/// players) and incrementally appending each subsequent move. It completely captures the game
/// state, which allows players to make moves based on an untrusted source of game state, such as a
/// preconfirmations feed (potentially enabling real time play). The engine simply discards any move
/// where the intended game state does not match that actual game state, so that a player cannot be
/// tricked into making an uninteded move.
#[derive(
    Clone, Copy, Debug, Display, FromStr, Deserialize, Serialize, AsRef, Into, PartialEq, Eq,
)]
#[display("{_0}")]
#[serde(transparent)]
pub struct GameHash(FixedBytes<32>);

#[derive(Clone, Debug, Display)]
pub enum Outcome {
    #[display("{winner} defeats {loser} by checkmate")]
    Checkmate { winner: Address, loser: Address },
    #[display("{winner} wins by resignation")]
    Resignation { winner: Address, loser: Address },
    #[display("the game ends in a draw due to stalemate")]
    Stalemate,
    #[display("the game ends in a draw due to insufficient material")]
    InsufficientMaterial,
    #[display("the game is drawn")]
    Draw,
}

impl Outcome {
    /// The addresses of the winning and losing players, if the outcome is decisive.
    pub fn winner_loser(&self) -> Option<(Address, Address)> {
        match self {
            Self::Checkmate { winner, loser } => Some((*winner, *loser)),
            Self::Resignation { winner, loser } => Some((*winner, *loser)),
            _ => None,
        }
    }

    pub fn is_victory(&self) -> bool {
        self.winner_loser().is_some()
    }

    pub fn is_draw(&self) -> bool {
        !self.is_victory()
    }
}

#[derive(Debug)]
pub struct Game {
    white: Address,
    black: Address,
    position: Chess,
    half_move: u16,
    id: GameId,
    hash: GameHash,
}

impl Game {
    /// Construct a new game in the starting position.
    pub fn new(id: GameId, white: Address, black: Address) -> Self {
        // Construct the hash of the initial game state.
        let mut bytes = id.0.to_le_bytes().to_vec();
        bytes.extend(white.0);
        bytes.extend(black.0);
        let hash = GameHash(keccak256(bytes));

        Self {
            white,
            black,
            position: Default::default(),
            half_move: 0,
            id,
            hash,
        }
    }

    /// Construct the game state resulting from the given moves (in SAN+ notation).
    pub fn from_moves(
        id: GameId,
        white: Address,
        black: Address,
        moves: impl IntoIterator<Item = San>,
    ) -> anyhow::Result<Self> {
        let mut game = Self::new(id, white, black);
        for san in moves {
            game.play_next_move(san)?;
        }
        Ok(game)
    }

    /// The ID of the game.
    pub fn id(&self) -> GameId {
        self.id
    }

    /// The hash of the current game state.
    pub fn hash(&self) -> GameHash {
        self.hash
    }

    /// The player controlling white.
    pub fn white(&self) -> Address {
        self.white
    }

    /// The player controlling black.
    pub fn black(&self) -> Address {
        self.black
    }

    /// The outcome of the game, if it is over.
    pub fn outcome(&self) -> Option<Outcome> {
        Some(match self.position.outcome()? {
            shakmaty::Outcome::Decisive { winner } => Outcome::Checkmate {
                winner: self.player(winner),
                loser: self.player(!winner),
            },
            shakmaty::Outcome::Draw => {
                if self.position.is_stalemate() {
                    Outcome::Stalemate
                } else if self.position.is_insufficient_material() {
                    Outcome::InsufficientMaterial
                } else {
                    Outcome::Draw
                }
            }
        })
    }

    /// A human-readable text representation of the current board state.
    pub fn ansi_board(&self, perspective: Color) -> String {
        let board = self.position.board();
        let mut ranks = (0..8).map(|rank| {
            let mut row = (0..8).map(|file| {
                let square = Square::from_coords(File::new(file), Rank::new(rank));
                let square_color = Color::from_white(square.is_light());
                let style = Style::new().on(ansi_color(square_color));
                match board.piece_at(square) {
                    Some(piece) => {
                        const PIECES: [[char; 6]; 2] = [
                            ['♟', '♞', '♝', '♜', '♛', '♚'],
                            ['♙', '♘', '♗', '♖', '♕', '♔'],
                        ];
                        style.fg(ansi_color(!square_color)).paint(format!(
                            " {} ",
                            PIECES[(piece.color == square_color) as usize][piece.role as usize - 1]
                        ))
                    }
                    None => style.paint("   "),
                }
                .to_string()
            });
            let row = if perspective == Color::White {
                row.join("")
            } else {
                row.rev().join("")
            };
            format!("{} {row}", rank + 1)
        });
        let (ranks, rank_labels) = if perspective == Color::White {
            (ranks.rev().join("\n"), " a  b  c  d  e  f  g  h ")
        } else {
            (ranks.join("\n"), " h  g  f  e  d  c  b  a ")
        };
        format!("{ranks}\n  {rank_labels}")
    }

    /// Get the color controlled by `player`, if they are playing in this game.
    pub fn player_color(&self, player: Address) -> Option<Color> {
        if self.white == player {
            Some(Color::White)
        } else if self.black == player {
            Some(Color::Black)
        } else {
            None
        }
    }

    /// Get the player controlling `color`.
    pub fn player(&self, color: Color) -> Address {
        match color {
            Color::White => self.white,
            Color::Black => self.black,
        }
    }

    /// Make a move as `player`, if legal.
    ///
    /// The move is given in SAN.
    pub fn play(
        &mut self,
        player: Address,
        expected_state: GameHash,
        san: San,
    ) -> anyhow::Result<Move> {
        let color = self
            .player_color(player)
            .context(format!("invalid player {player}"))?;
        ensure!(self.position.turn() == color, "it is not {color}'s turn");
        ensure!(
            expected_state == self.hash,
            "the current state {} does not match the intended state {expected_state}",
            self.hash
        );
        self.play_next_move(san)
    }

    pub fn play_next_move(&mut self, san: San) -> anyhow::Result<Move> {
        // Make the move.
        let m = san.to_move(&self.position)?;
        self.position = std::mem::take(&mut self.position).play(&m)?;
        self.half_move += 1;

        // Construct the canonical notation for the move.
        let suffix = if self.position.is_checkmate() {
            Some(Suffix::Checkmate)
        } else if self.position.is_check() {
            Some(Suffix::Check)
        } else {
            None
        };
        let notation = SanPlus { san, suffix };

        // Update the game hash.
        let mut bytes = self.hash.0 .0.to_vec();
        bytes.extend(notation.to_string().as_bytes());
        self.hash = GameHash(keccak256(bytes));

        Ok(Move {
            san: notation,
            half_move: self.half_move,
        })
    }

    pub fn half_move(&self) -> u16 {
        self.half_move
    }

    pub fn full_move(&self) -> u16 {
        self.half_move / 2
    }

    pub fn turn(&self) -> Color {
        self.position.turn()
    }
}

#[derive(Clone, Debug)]
pub struct Move {
    san: SanPlus,
    half_move: u16,
}

impl Move {
    pub fn half_move(&self) -> u16 {
        self.half_move
    }

    pub fn san(&self) -> String {
        self.san.to_string()
    }
}

fn ansi_color(color: Color) -> ansi_term::Color {
    match color {
        Color::White => ansi_term::Color::White,
        Color::Black => ansi_term::Color::Black,
    }
}
