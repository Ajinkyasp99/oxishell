//! oxishell desktop application.
fn main() -> gtk4::glib::ExitCode {
    // Inside Oxishell.app, use the GTK data copied into the bundle.
    // Started from Finder or the Dock, the app gets launchd's minimal PATH;
    // take the user's login-shell PATH so git, docker and editors are found.
    // Before GTK starts any thread.
    #[cfg(target_os = "macos")]
    {
        oxishell::platform::macos::setup_bundle_env();
        oxishell::platform::macos::import_login_path();
    }
    oxishell::ui::run()
}
