use serde::{
    Serialize,
    Deserialize,
};

#[derive(Serialize, Deserialize)]
pub enum U2SPost {
    Auth {
        username: String,
        password: String,
    },
}

#[derive(Serialize, Deserialize)]
pub enum U2SGet {
    InitialSync,
}
