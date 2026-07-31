use std::process::Command;

#[test]
fn video_commands_are_not_exposed() {
    for command in ["video", "vmaf"] {
        let output = Command::new(env!("CARGO_BIN_EXE_media-forge"))
            .arg(command)
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    }
}
