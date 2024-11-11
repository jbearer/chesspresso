use alloy::primitives::Address;
use chesspresso_core::{
    game::{GameId, San},
    message::{Game, UserStats},
};
use futures::{future::Future, stream::Stream};

pub mod inspect;

pub use self::inspect::InspectIndexer;

pub trait Indexer {
    fn games_with_user(
        &self,
        address: Address,
        after: Option<GameId>,
    ) -> impl Stream<Item = Game> + Send + Unpin;
    fn moves(&self, id: GameId, from: u16) -> impl Stream<Item = San> + Send + Unpin;
    fn user_stats(
        &self,
        address: Address,
    ) -> impl Future<Output = anyhow::Result<UserStats>> + Send;
}
