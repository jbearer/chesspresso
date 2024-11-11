use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::{coins_bip39::English, MnemonicBuilder},
    sol_types::sol,
    transports::http::{Client, Http},
};
use anyhow::{ensure, Context};
use chesspresso_core::{
    db::Db,
    game::{GameId, San},
    message::Advance,
};
use chesspresso_indexer::{Indexer, InspectIndexer};
use clap::{Parser, Subcommand};
use futures::stream::TryStreamExt;
use std::path::{Path, PathBuf};
use std::{env, process::exit};
use url::Url;

sol! {
    #![sol(alloy_sol_types = alloy::sol_types)]

    contract InputBox {
        function addInput(address dapp, bytes payload);
    }
}

/// Chesspresso -- play chess on Espresso!
///
/// Powered by Cartesi and Espresso Systems
#[derive(Parser)]
struct Options {
    /// The mnemonic phrase to use to generate a wallet for signing messages.
    #[clap(short, long, env = "CHESSPRESSO_MNEMONIC")]
    mnemonic: String,

    /// The account to use for signing messages.
    #[clap(
        short = 'i',
        long,
        env = "CHESSPRESSO_ACCOUNT_INDEX",
        default_value = "0"
    )]
    account_index: u32,

    #[clap(short, long, env = "CHESSPRESSO_DB")]
    db: Option<PathBuf>,

    /// Base layer RPC.
    #[clap(
        short,
        long,
        env = "CHESSPRESSO_RPC",
        default_value = "http://localhost:8545"
    )]
    rpc: Url,

    /// Chesspresso dApp contract address.
    #[clap(
        long,
        env = "CHESSPRESSO_DAPP_ADDRESS",
        default_value = "0xab7528bb862fB57E8A2BCd567a2e929a0Be56a5e"
    )]
    dapp_address: Address,

    /// InputBox contract address.
    #[clap(
        long,
        env = "CHESSPRESSO_INPUT_BOX_ADDRESS",
        default_value = "0x59b22D57D4f067708AB0c00552767405926dc768"
    )]
    input_box_address: Address,

    /// Confirmations required before considering a transaction successful.
    #[clap(short, long, env = "CHESSPRESSO_CONFIRMATIONS", default_value = "1")]
    confirmations: u64,

    /// Endpoint for a Chesspresso indexer.
    #[clap(
        long,
        env = "CHESSPRESSO_INDEXER",
        default_value = "http://localhost:8080"
    )]
    indexer: Url,

    #[clap(subcommand)]
    command: Command,
}

impl Options {
    fn provider(&self) -> anyhow::Result<(Address, impl Provider<Http<Client>>)> {
        let signer = MnemonicBuilder::<English>::default()
            .phrase(&self.mnemonic)
            .index(self.account_index)?
            .build()?;
        let address = signer.address();
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(EthereumWallet::new(signer))
            .on_http(self.rpc.clone());
        Ok((address, provider))
    }

    async fn db(&self, address: Address) -> anyhow::Result<Db> {
        let db_path = match &self.db {
            Some(path) => path,
            None => &Path::new(&env::var("HOME")?).join(format!(".chesspresso/{address}.sqlite")),
        };
        Db::open(db_path).await
    }
}

#[derive(Subcommand)]
enum Command {
    /// Print address.
    Address,

    /// List games.
    Games,

    /// Show the position for a game.
    Game { id: GameId },

    /// Challenge someone to a game.
    Challenge {
        opponent: Address,
        first_move: Option<San>,
    },

    /// Make a move.
    Play { id: GameId, san: San },

    /// Resign a game.
    Resign { id: GameId },

    /// Get user stats.
    Stats { user: Option<Address> },
}

impl Command {
    async fn run(
        &self,
        opt: &Options,
        address: Address,
        provider: &impl Provider<Http<Client>>,
        indexer: &impl Indexer,
        db: &mut Db,
    ) -> anyhow::Result<()> {
        match self {
            Self::Address => println!("{address}"),
            Self::Games => {
                let games: Vec<_> = db.games(address, None).try_collect().await?;
                for game in games {
                    let id = game.id;
                    let game = db
                        .game(game.id)
                        .await
                        .context(format!("loading game {id}"))?;
                    let color = game
                        .player_color(address)
                        .context(format!("not playing in game {id}"))?;
                    let opponent = game.player(!color);
                    let move_ = game.full_move() + 1;
                    let whose = if game.turn() == color {
                        "your"
                    } else {
                        "their"
                    };
                    println!("{id}. as {color} vs. {opponent} (move {move_}, {whose} move)",);
                }
            }
            Self::Game { id } => {
                let game = db.game(*id).await?;
                let color = game
                    .player_color(address)
                    .context(format!("not playing in game {id}"))?;
                let moves = db.game_notation(*id).await?;
                println!(
                    "{moves}\n\n{}\n\n{} to move",
                    game.ansi_board(color),
                    game.turn()
                );
            }
            Self::Challenge {
                opponent,
                first_move,
            } => {
                advance(
                    opt,
                    provider,
                    Advance::Challenge {
                        opponent: *opponent,
                        first_move: first_move.as_ref().map(|san| san.to_string()),
                    },
                )
                .await?;
            }
            Self::Play { id, san } => {
                let game = db.game(*id).await?;
                ensure!(address == game.player(game.turn()), "it is not your turn");

                advance(
                    opt,
                    provider,
                    Advance::Move {
                        id: *id,
                        hash: game.hash(),
                        san: san.to_string(),
                    },
                )
                .await?;
            }
            Self::Resign { id } => {
                let game = db.game(*id).await?;
                advance(
                    opt,
                    provider,
                    Advance::Resign {
                        id: *id,
                        hash: game.hash(),
                    },
                )
                .await?;
            }
            Self::Stats { user } => {
                let stats = indexer.user_stats(user.unwrap_or(address)).await?;
                println!("{stats:#?}");
            }
        }

        Ok(())
    }
}

async fn advance(
    opt: &Options,
    provider: &impl Provider<Http<Client>>,
    message: Advance,
) -> anyhow::Result<()> {
    let data = serde_json::to_string(&message)?;
    let tx = TransactionRequest::default()
        .with_call(&InputBox::addInputCall {
            dapp: opt.dapp_address,
            payload: data.as_bytes().to_vec().into(),
        })
        .with_to(opt.input_box_address);
    provider
        .send_transaction(tx)
        .await?
        .with_required_confirmations(opt.confirmations)
        .watch()
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let opt = Options::parse();

    let (address, provider) = match opt.provider() {
        Ok(res) => res,
        Err(err) => {
            eprintln!("failed to connect to base layer: {err:#}");
            exit(1);
        }
    };

    let mut db = match opt.db(address).await {
        Ok(db) => db,
        Err(err) => {
            eprintln!("failed to open local database: {err:#}");
            exit(1);
        }
    };

    let indexer = InspectIndexer::new(opt.indexer.clone());

    if let Err(err) = opt
        .command
        .run(&opt, address, &provider, &indexer, &mut db)
        .await
    {
        eprintln!("{err:#}");
        exit(1);
    }
}
