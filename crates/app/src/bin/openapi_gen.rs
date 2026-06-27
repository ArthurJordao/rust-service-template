fn main() {
    let doc = app::openapi::api_doc();
    println!("{}", doc.to_pretty_json().expect("serialize openapi"));
}
