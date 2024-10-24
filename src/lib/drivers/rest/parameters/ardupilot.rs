use std::collections::HashMap;

use cached::proc_macro::cached;
use include_dir::{include_dir, Dir};
use serde_json::Value;

static PROJECT_DIR: Dir = include_dir!("src/lib/drivers/rest/parameters/ardupilot_parameters");

#[cached]
pub fn parameters_dict() -> HashMap<String, HashMap<String, Value>> {
    let mut map = HashMap::new();

    // Firmware + Version
    for dirs in PROJECT_DIR.dirs() {
        // Parameters file
        for file in dirs.files() {
            if !file.path().to_string_lossy().to_string().contains("apm.pdef.json") {
                continue
            }

            let folder_key = file.path().parent().unwrap().to_string_lossy().to_string();
            dbg!(&folder_key);
            let mut split_key = folder_key.split("-");
            let firmware = split_key.nth(0);
            let version = split_key.nth(0);
            if let (Some(version), Some(firmware)) = (version, firmware) {
                if let Ok(json_content) = serde_json::from_str(file.contents_utf8().unwrap()) {
                    map.entry(firmware.to_string()).or_insert_with(||
                        HashMap::from([(version.to_string(), json_content)])
                    );
                }
            }

            break;
        }
    }

    map
}