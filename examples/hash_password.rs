//! Print Argon2id hashes for the given passwords.
//!
//! ```bash
//! cargo run --example hash_password -- admin123 lovemefan123
//! ```
//!
//! Use this to regenerate the seed data in `mysql/create_database.sql`.

fn main() {
    let passwords: Vec<String> = std::env::args().skip(1).collect();
    if passwords.is_empty() {
        eprintln!("usage: cargo run --example hash_password -- <password> [password...]");
        std::process::exit(2);
    }
    for password in passwords {
        match sanicboot::utils::hash::hash_password(&password) {
            Ok(hash) => println!("{password}\t{hash}"),
            Err(err) => eprintln!("{password}\tERROR: {err}"),
        }
    }
}
