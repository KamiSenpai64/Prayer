use mp4ameta::{Tag, Data};

fn main() {
    let mut tag = Tag::read_from_path("test.m4a").unwrap_or_else(|_| Tag::default());
    tag.set_title("Test Title");
    tag.set_artist("Test Artist");
    tag.set_album("Test Album");
    tag.set_year("2020");
    tag.set_track_number(5);
    tag.write_to_path("test.m4a").unwrap();
    
    let tag2 = Tag::read_from_path("test.m4a").unwrap();
    println!("Title: {:?}", tag2.title());
    println!("Artist: {:?}", tag2.artist());
    println!("Album: {:?}", tag2.album());
    println!("Year: {:?}", tag2.year());
    println!("Track: {:?}", tag2.track_number());
}
