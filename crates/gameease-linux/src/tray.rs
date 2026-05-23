use anyhow::Result;
use ksni::blocking::TrayMethods;

/// Starts the GameEase tray indicator for the lifetime of the process.
pub fn spawn_tray_icon() -> Result<()> {
    let handle = GameEaseTray.assume_sni_available(true).spawn()?;
    std::mem::forget(handle);

    Ok(())
}

struct GameEaseTray;

impl ksni::Tray for GameEaseTray {
    fn id(&self) -> String {
        "gameease".into()
    }

    fn title(&self) -> String {
        "GameEase".into()
    }

    fn icon_name(&self) -> String {
        "input-gaming".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "GameEase".into(),
            description: "GameEase overlay is running".into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        vec![
            StandardItem {
                label: "GameEase is running".into(),
                enabled: false,
                icon_name: "input-gaming".into(),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}
