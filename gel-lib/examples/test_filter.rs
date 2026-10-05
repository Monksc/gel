fn main() {
    let svg = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let result = gel::strip_known_artifacts(&svg).unwrap();
    eprintln!("elements stripped: {}", result.elements_stripped);
    std::fs::write(std::env::args().nth(2).unwrap(), result.svg).unwrap();
}
