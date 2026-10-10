use genoryn_core::{chain::Chain, params::Network};

#[test]
fn exclusive_database_owner_and_network_schema_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain.sqlite");
    let chain = Chain::open(&path, Network::Devnet).unwrap();
    assert!(Chain::open(&path, Network::Devnet).is_err());
    drop(chain);
    assert!(Chain::open(&path, Network::Testnet).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE meta SET value='999' WHERE key='schema'", [])
        .unwrap();
    drop(db);
    assert!(Chain::open(&path, Network::Devnet).is_err());
}

#[test]
fn corrupted_database_does_not_silently_initialize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.sqlite");
    std::fs::write(&path, b"not a database").unwrap();
    assert!(Chain::open(&path, Network::Devnet).is_err());
}
