use alloy::sol_types::sol;
use serde::{Deserialize, Serialize};

sol! {
    #![sol(alloy_sol_types = alloy::sol_types)]

    #[derive(Debug, Deserialize, Serialize)]
    event Victory(
        int32 id,
        address winner,
        address loser,
        string message,
        string notation,
    );
}
