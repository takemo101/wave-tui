set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

bin := "wave-tui"
plugin_id := "wave-tui.radio"
install_dir := env_var_or_default("INSTALL_DIR", env_var("HOME") / ".local" / "bin")

# Show available recipes
_default:
    @just --list

# Build the release binary
build-release:
    cargo build --release

# Open the installed wave-tui plugin in its dedicated Herdr tab.
herdr-open: build-release
    herdr plugin pane open --plugin "{{plugin_id}}" --entrypoint radio --placement tab --focus

# Link this checkout as the Herdr plugin, build it, and open its dedicated tab.
herdr-dev: build-release
    herdr plugin link "{{justfile_directory()}}"
    herdr plugin pane open --plugin "{{plugin_id}}" --entrypoint radio --placement tab --focus

# Install wave-tui to INSTALL_DIR (default: ~/.local/bin)
install: build-release
    mkdir -p "{{install_dir}}"
    install -m 0755 "target/release/{{bin}}" "{{install_dir}}/{{bin}}"
    @echo "Installed {{bin}} to {{install_dir}}/{{bin}}"
    @echo "Ensure {{install_dir}} is on your PATH."

# Remove wave-tui from INSTALL_DIR (default: ~/.local/bin)
uninstall:
    rm -f "{{install_dir}}/{{bin}}"
    @echo "Removed {{bin}} from {{install_dir}}/{{bin}}"
