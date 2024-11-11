CREATE TABLE user (
    address VARCHAR PRIMARY KEY,

    elo_value REAL,
    elo_deviation REAL,
    elo_volatility REAL,

    white_wins INT,
    white_losses INT,
    white_draws INT,
    black_wins INT,
    black_losses INT,
    black_draws INT
);

CREATE TABLE game (
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- The address of the player playing as white.
    white VARCHAR NOT NULL,
    -- The address of the player playing as black.
    black VARCHAR NOT NULL
);

CREATE TABLE move (
    game INT NOT NULL REFERENCES game (id) ON DELETE CASCADE,

    -- The half-move index. Odd numbers are white moves; even are black. E.g. index 1 is move 1.,
    -- index 2 is move 1. ....
    half_move INT NOT NULL,

    -- The move in SAN+.
    san VARCHAR NOT NULL,

    PRIMARY KEY (game, half_move)
);
