use std::fs::{self, File};
use std::io::{Write, BufWriter};
use std::fmt::Write as _;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Platform {
    #[serde(default)]
    motors: Vec<Motor>,

    #[serde(default)]
    motor_pairs: Vec<MotorPair>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Motor {
    name: String,
    gpio: (usize, usize),
}

#[derive(Debug, Deserialize)]
struct MotorPair {
    name: String,
    left: String,
    right: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Necessary because of this issue: https://github.com/rust-lang/cargo/issues/9641
    embuild::build::CfgArgs::output_propagated("ESP_IDF")?;
    embuild::build::LinkArgs::output_propagated("ESP_IDF")?;

    // generate static info about this build
    {
        let mut f = BufWriter::new(File::create("src/meta.rs").unwrap());
        writeln!(f, "pub const DEFAULT_CLIENT_ID: &'static str = \"esp-{}\";", names::Generator::default().next().unwrap()).unwrap();
    }

    {
        let platform = {
            let path = std::env::var("PLATFORM").expect("missing PLATFORM env var (see platforms/ for examples)");
            let content = fs::read_to_string(path).expect("failed to read PLATFORM toml file");
            toml::from_str::<Platform>(&content).expect("failed to parse PLATFORM toml file")
        };

        let mut ledc_channel = 0;

        let mut objects = String::new();
        let mut handlers = String::new();
        let mut syscalls = String::new();

        if !platform.motors.is_empty() {
            syscalls += " SyscallMenu::Submenu {{ label: \"motors\", content: [\n";

            for motor in platform.motors.iter() {
                write!(objects, r#"
                    #[allow(non_snake_case)] let motor_{name} = RefCell::new(MotorController {{
                        positive: LedcDriver::new(peripherals.ledc.channel{pos_channel}, pwm_timer.clone(), peripherals.pins.gpio{pos_gpio}).unwrap(),
                        negative: LedcDriver::new(peripherals.ledc.channel{neg_channel}, pwm_timer.clone(), peripherals.pins.gpio{neg_gpio}).unwrap(),
                    }});"#, name = motor.name, pos_gpio = motor.gpio.0, neg_gpio = motor.gpio.1, pos_channel = ledc_channel, neg_channel = ledc_channel + 1).unwrap();
                ledc_channel += 2;

                write!(handlers, r#"
                    "driveMotor{name}" => {{
                        match args.as_slice() {{
                            [x] => match x.to_number() {{
                                Ok(x) => {{
                                    motor_{name}.borrow_mut().set_power(x).unwrap();
                                    key.complete(Ok(Intermediate::Json(json!("OK"))));
                                }}
                                Err(_) => key.complete(Err(format!("drive{name} expected a number, got {{:?}}", x.get_type()))),
                            }}
                            _ => key.complete(Err(format!("drive{name} expected 1 arg, got {{}}", args.len()))),
                        }}
                        RequestStatus::Handled
                    }}"#, name = motor.name).unwrap();

                write!(syscalls, r#"
                    SyscallMenu::Entry {{ label: "drive{name}" }},
                "#, name = motor.name).unwrap();
            }

            for pair in platform.motor_pairs.iter() {
                write!(handlers, r#"
                    "drivePair
                "#, name = pair.name, left = pair.left, right = pair.right).unwrap();
            }

            syscalls += "] },\n";
        } else {
            assert!(platform.motor_pairs.is_empty());
        }

        let mut f = BufWriter::new(File::create("src/platform.rs").unwrap());
        writeln!(f, include_str!("templates/platform.rs"), objects = objects, handlers = handlers, syscalls = syscalls).unwrap();
    }

    Ok(())
}
