use std::fs::File;
use std::io::{Write, BufWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Necessary because of this issue: https://github.com/rust-lang/cargo/issues/9641
    embuild::build::CfgArgs::output_propagated("ESP_IDF")?;
    embuild::build::LinkArgs::output_propagated("ESP_IDF")?;

    // generate static info about this build
    let mut f = BufWriter::new(File::create("src/meta.rs").unwrap());
    writeln!(f, "pub const NAME: &'static str = \"esp-{}\";", names::Generator::default().next().unwrap()).unwrap();

    Ok(())
}
