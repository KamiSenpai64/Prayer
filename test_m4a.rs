use audiotags::Tag;
use std::fs::File;

fn main() {
    let path = "test.m4a";
    File::create(path).unwrap();
    let mut tag = Tag::default();
    tag.set_title("Test");
    match tag.write_to_path(path) {
        Ok(_) => println!("Success!"),
        Err(e) => println!("Error: {:?}", e),
    }
}
