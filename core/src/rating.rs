use glicko2::{GameResult, Glicko2Rating};

/// The system constant instantiating the Glicko2 rating system.
///
/// Glicko2 is parameterized by a constant which controls how significantly ratings change with each
/// result. Recommended parameters are in the range 0.3 to 1.2, with lower values causing less
/// volatility.
const SYSTEM_CONSTANT: f64 = 0.8;

pub fn update(rating: Glicko2Rating, result: GameResult) -> Glicko2Rating {
    glicko2::new_rating(rating, &[result], SYSTEM_CONSTANT)
}

pub fn unrated() -> Glicko2Rating {
    Glicko2Rating::unrated()
}
