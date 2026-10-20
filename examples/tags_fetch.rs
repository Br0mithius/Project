fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let db = msh_deckbuilder::db::CardDb::load(&dir.join("cards.json")).unwrap();
    println!("{:?}", msh_deckbuilder::tags::fetch(&dir, &db));
    let mut db = db;
    db.attach_ratings(&dir);
    let tagged: Vec<_> = db
        .cards
        .iter()
        .filter(|c| !c.tags.is_empty())
        .take(5)
        .map(|c| (c.name.clone(), c.tags.clone(), c.roles()))
        .collect();
    for t in tagged {
        println!("{:?}", t);
    }
}
