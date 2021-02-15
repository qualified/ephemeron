// Output CRD
// cargo run --bin crd
use ephemeron::Ephemeron;

fn main() {
    println!("{}", serde_yaml::to_string(&Ephemeron::crd()).unwrap())
}
