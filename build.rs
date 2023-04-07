use std::fs::{self, File};
use std::io::{Write, BufWriter};

use tera::{Tera, Context};

use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all(deserialize = "kebab-case"))]
struct Platform {
    #[serde(default)]
    motors: Vec<Motor>,

    #[serde(default)]
    motor_groups: Vec<MotorGroup>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all(deserialize = "kebab-case"))]
struct Motor {
    name: String,
    gpio: (usize, usize),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all(deserialize = "kebab-case"))]
struct MotorGroup {
    name: String,
    motors: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Necessary because of this issue: https://github.com/rust-lang/cargo/issues/9641
    embuild::build::CfgArgs::output_propagated("ESP_IDF")?;
    embuild::build::LinkArgs::output_propagated("ESP_IDF")?;

    let templates = Tera::new("templates/**/*").unwrap();

    // generate static info about this build
    {
        let mut f = BufWriter::new(File::create("src/meta.rs").unwrap());
        writeln!(f, "pub const DEFAULT_CLIENT_ID: &'static str = \"esp-{}\";", names::Generator::default().next().unwrap()).unwrap();
    }

    {
        let mut platform = {
            let content = fs::read_to_string("platform.toml").expect("failed to read platform.toml");
            toml::from_str::<Platform>(&content).expect("failed to parse platform.toml")
        };

        for motor in platform.motors.iter() {
            platform.motor_groups.push(MotorGroup { name: motor.name.clone(), motors: vec![motor.name.clone()] });
        }

        let mut f = BufWriter::new(File::create("src/platform.rs").unwrap());
        templates.render_to("platform.rs", &Context::from_serialize(&platform).unwrap(), &mut f).unwrap();
    }

    Ok(())
}
