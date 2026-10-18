//! binary "replay" — padanan `node replay.js <dir> <fromInit> <endBlock> <out.jsonl> [ethUsd|auto]`
fn main() {
    std::process::exit(port_engine::replay::main_cli());
}
