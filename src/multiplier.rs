use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::fmt::Formatter;
use strum_macros::EnumIter;

#[derive(Clone, Debug)]
pub struct Multipliers(pub [MultiplierNote; 11]);

impl Multipliers {
    pub fn get_multiplier_note(&self, note_id: &str) -> Option<MultiplierNote> {
        self.0
            .iter()
            .find(|multiplier| multiplier.note_id == note_id)
            .cloned()
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MultiplierNote {
    pub multiplier: Multiplier,
    pub note_id: String,
}

impl fmt::Display for MultiplierNote {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format!("{}, {}", self.note_id, self.multiplier.get_content()).fmt(f)
    }
}

#[derive(Clone, Serialize, Deserialize, EnumIter, Debug)]
pub enum Multiplier {
    X1_05,
    X1_1,
    X1_33,
    X1_5,
    X2,
    X3,
    X10,
    X25,
    X50,
    X100,
    X1000,
}

impl Multiplier {
    pub const fn get_multiplier(&self) -> f32 {
        match self {
            Multiplier::X1_05 => 1.05,
            Multiplier::X1_1 => 1.10,
            Multiplier::X1_33 => 1.33,
            Multiplier::X1_5 => 1.5,
            Multiplier::X2 => 2.0,
            Multiplier::X3 => 3.0,
            Multiplier::X10 => 10.0,
            Multiplier::X25 => 25.0,
            Multiplier::X50 => 50.0,
            Multiplier::X100 => 100.0,
            Multiplier::X1000 => 1000.0,
        }
    }

    pub const fn get_lower_than(&self) -> u16 {
        match self {
            Multiplier::X1_05 => 60_541,
            Multiplier::X1_1 => 57_789,
            Multiplier::X1_33 => 47_796,
            Multiplier::X1_5 => 42_379,
            Multiplier::X2 => 31_784,
            Multiplier::X3 => 21_189,
            Multiplier::X10 => 6_356,
            Multiplier::X25 => 2_542,
            Multiplier::X50 => 1_271,
            Multiplier::X100 => 635,
            Multiplier::X1000 => 64,
        }
    }

    pub fn get_content(&self) -> String {
        match self {
            Multiplier::X1_05 => "1.05x".to_string(),
            Multiplier::X1_1 => "1.1x".to_string(),
            Multiplier::X1_33 => "1.33x".to_string(),
            Multiplier::X1_5 => "1.5x".to_string(),
            Multiplier::X2 => "2x".to_string(),
            Multiplier::X3 => "3x".to_string(),
            Multiplier::X10 => "10x".to_string(),
            Multiplier::X25 => "25x".to_string(),
            Multiplier::X50 => "50x".to_string(),
            Multiplier::X100 => "100x".to_string(),
            Multiplier::X1000 => "1000x".to_string(),
        }
    }
}
