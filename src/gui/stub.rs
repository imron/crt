use anyhow::Result;

pub fn run_gui(
    _base: Option<&str>,
    _root: bool,
    _reset: bool,
    _standalone: bool,
    _foreground: bool,
) -> Result<()> {
    anyhow::bail!("`crt gui` is only available on macOS")
}
