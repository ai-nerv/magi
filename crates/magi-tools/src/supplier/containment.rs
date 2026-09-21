use super::*;
use crate::holding::{Context, Holds};
use std::os::unix::fs::PermissionsExt;

#[derive(Default)]
struct Recorded(Mutex<Vec<Context>>);

impl Holds for Recorded {
    fn hold(
        &self,
        _: &str,
        _: &magi_proto::tooling::Surface,
        _: &serde_json::Value,
        context: &Context,
    ) -> Option<String> {
        self.0.lock().expect("recording").push(context.clone());
        None
    }
}

#[test]
fn a_tool_and_its_surface_receive_the_same_captured_policy() {
    let dir = magi_model::scratch::Scratch::new("magi-supplier", "surface-policy");
    let script = dir.join("supplier");
    let reply = serde_json::json!({ "ok": true, "result": [{
        "shown": { "shown": "surface", "rows": 4, "about": "probe" }
    }] });
    std::fs::write(&script, format!(
        "#!/bin/sh\n/bin/cat > /dev/null\nprintf '%s\\n' \"$CASPER_JAIL\" \"$CASPER_CONFIGURE\" \"$MAGI_TOOLS_CONFIGURE\" \"$PWD\" > report\nprintf '%s\\n' '{reply}'\n"
    )).expect("fixture script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    for isolation in [false, true] {
        let recorded = Arc::new(Recorded::default());
        let supplied = SuppliedTool {
            card: serde_json::from_value(serde_json::json!({
                "name": "probe", "description": "probe", "parameters": {}
            }))
            .expect("card"),
            program: script.display().to_string(),
            asks: Arc::new(crate::question::Unanswered),
            holds: recorded.clone(),
            knows: Arc::new(crate::holding::Incurious),
            configured: "coordinator-settings".into(),
        };
        let ops = crate::ops::Real::new(dir.to_path_buf()).isolating(isolation);
        let _ = supplied.run(
            &serde_json::json!({ "CASPER_JAIL": "", "cwd": "/", "reach": true }),
            &ops,
            &crate::Uncancelled,
        );
        let held = recorded.0.lock().expect("recording");
        assert_eq!(held.len(), 1, "surface was not reached");
        let context = &held[0];
        assert_eq!(context.jail.is_some(), isolation);
        assert_eq!(context.configure, "coordinator-settings");
        assert_eq!(context.cwd.as_deref(), Some(&*dir));
        assert_eq!(
            std::fs::read_to_string(dir.join("report")).expect("spawn report"),
            format!(
                "{}\ncoordinator-settings\ncoordinator-settings\n{}\n",
                context.jail.as_deref().unwrap_or(""),
                dir.display()
            )
        );
        if let Some(jail) = &context.jail {
            let profile: serde_json::Value = serde_json::from_str(jail).expect("profile");
            if let Some(tmp) = profile["tmp"].as_str() {
                let _ = std::fs::remove_dir_all(tmp);
            }
        }
    }
}
