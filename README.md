# Chesspresso

Play chess on the blockchain, powered by Espresso and Cartesi.

![Espresso Systems](espresso.svg)
![Cartesi Project](cartesi.png)

* Cartesi's rollup development platform makes it a snap to run a chess engine an a
  verifiable-on-chain virtual machine. The core Chesspresso dApp is written in Rust using an
  off-the-shelf, non-web3-specific chess crate,
  [shakmaty](https://docs.rs/shakmaty/latest/shakmaty/).
* The Espresso network provides fast, secure confirmations, making it possible to play onchain in
  real time, with classical or even rapid time controls.
* The Cartesi machine provides even faster, centralized preconfirmations, enabling play with blitz
  or bullet time controls. The design of the Chesspresso dApp allows users to play based of an
  untrusted stream of preconfirmations by including a hash of the current game state in each state
  update. The Chesspresso dApp then rejects any update where the intended hash doesn't match the
  actual current game hash (such as if the untrusted preconfirmations server lied about the state).
  This consistency guarantee is ultimately enforced by the base chain via Cartesi's fraud proof
  system [Dave](https://github.com/cartesi/dave).
* Espresso's DA layer makes it fast and cheap to store each individual chess move onchain as a
  separate transaction, further simplifying development, since no peer-to-peer communication is
  required except through Espresso's public network.

## Features

* Challenge your friends to onchain correspondence chess
* Mint an NFT whenever you win a game
* Keep track of statistics like games won and ELO ratings

## Future Features

* Puzzles: mint a collectible NFT by being among the first to solve a puzzle. The longer the puzzle
  goes unsolved, the more valuable the collectible!
* Time controls
* GUI

## Development

### Setup

* Install [the Cartesi CLI](https://docs.cartesi.io/cartesi-rollups/1.5/quickstart/)
* Install [rustup](https://rustup.rs/) and Rust version 1.82
* Install Docker with the `buildx` plugin

### Running locally

* Build the dApp: `cartesi build`
* Start the dApp: `cartesi run`
* Start a client daemon for each user:
  `cargo run --release --bin chesspresso-client -- -u http://localhost:8080 -a <address>`.
  For the local demo, the first few accounts of the `test test test test test test test test test test test junk`
  mnemonic are funded, e.g. `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` and 
  `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`
* Use the Chesspresso CLI to interact:
  ```
  export CHESSPRESSO_MNEMONIC="test test test test test test test test test test test junk"
  cargo run --release --bin chesspresso -- -i <account-index> <subcommand>
  ```

  Useful sub-commands include:
  - `challenge <address> [first-move]`: challenge another player to a game, and optionally make the
  	first move (claiming white for yourself). E.g.
  	```
  	cargo run --release --bin chesspresso -- -i 0 challenge 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 e4
  	```
  - `games`: list your games
  - `game <i>`: show the current state of a given game
  - `play <i> <move>`: make a move (given in SAN notation) in a given game
  - `resign <i>`: resign game `i`
