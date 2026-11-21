//! Names and folders Dusty never touches, whatever the catalog says. Checked again right before
//! every removal (see `clean`).

use super::*;

/// Folder names whose caches hold real data or belong to sensitive apps: never swept.
/// Adapted from Mole's DATA_PROTECTED_BUNDLES / SYSTEM_CRITICAL_BUNDLES and hard-coded cache exceptions.
pub(crate) const PROTECTED_NAMES: &[&str] = &[
    // music & media libraries with offline content
    "com.spotify.client", "Spotify", "com.apple.Music", "com.apple.podcasts", "com.apple.iBooksX", "com.apple.iBooks", "com.apple.BKAgentService",
    // password managers, auth, keychains
    "com.1password.*", "com.agilebits.*", "1Password*", "com.lastpass.*", "com.dashlane.*", "com.bitwarden.*", "Bitwarden", "com.keepassx.*", "org.keepassxc.*",
    "com.authy.*", "com.yubico.*", "*keychain*", "*Keychain*", "com.proton.pass*", "*ProtonPass*", "Enpass*", "in.sinew.*",
    "com.nordpass.*", "com.keepersecurity.*",
    // cloud sync & backup
    "com.apple.bird", "com.apple.CloudDocs*", "CloudDocs", "com.dropbox.*", "com.getdropbox.*", "*Dropbox*", "com.google.GoogleDrive", "*GoogleDrive*", "Google Drive",
    "com.microsoft.OneDrive*", "*OneDrive*", "com.box.desktop*", "com.backblaze.*", "*Backblaze*", "com.synology.*", "com.amazon.drive",
    "com.nextcloud.*", "Nextcloud", "com.owncloud.*", "*pCloud*", "mega.mac*", "MEGAsync", "com.seafile.*", "com.resilio.*", "*Syncthing*",
    // VPNs & network tools
    "*tailscale*", "io.tailscale.*", "com.wireguard.*", "*WireGuard*", "*openvpn*", "*nordvpn*", "*expressvpn*", "*protonvpn*", "*mullvad*", "*surfshark*",
    "*cloudflare*warp*", "*zerotier*", "*clash*", "*Clash*", "com.nssurge.*",
    "PanGPS", "com.paloaltonetworks.*", "*GlobalProtect*", "com.cisco.*anyconnect*", "*AnyConnect*", "com.fortinet.*", "*FortiClient*",
    "*windscribe*", "com.privateinternetaccess.*",
    // input methods
    "*inputmethod*", "*InputMethod*", "com.apple.inputmethod.*", "im.rime.*",
    // editors, IDEs, AI apps keep indexes and conversation state here
    "com.jetbrains.*", "JetBrains*", "com.microsoft.VSCode*", "Code", "Code - Insiders", "Cursor", "Windsurf", "com.exafunction.*", "VSCodium",
    "com.vscodium*", "com.todesktop.*", "Zed", "dev.zed.*", "com.anthropic.*", "Claude", "com.openai.codex", "Codex",
    "com.ollama.ollama", "Ollama", "com.lmstudio.lmstudio", "LM Studio", "lm-studio",
    // virtualization & containers (live disk images)
    "com.docker.docker", "Docker Desktop", "dev.orbstack.*", "OrbStack", "com.utmapp.UTM", "com.parallels.*", "com.vmware.fusion", "org.virtualbox.*",
    "rancher-desktop", "io.rancherdesktop.*", "com.github.containers.*", "Podman Desktop", "com.vmware.*",
    // audio plugins, licenses
    "com.native-instruments.*", "com.paceap.*", "com.izotope.*", "iZotope", "com.avid.*",
    // browser automation runtimes that other tools depend on
    "ms-playwright", "Cypress", "camoufox", "puppeteer",
    // system components
    "com.apple.HomeKit", "com.apple.homed", "FamilyCircle", "com.apple.containermanagerd", "com.apple.ap.adprivacyd", "GeoServices",
    "CloudKit", "com.apple.akd", "com.apple.nsurlsessiond", "com.apple.coreaudio*", "*coreaudiod*",
    // other cleaners (avoid fighting them) and Sparkle updaters mid-download
    "com.macpaw.*", "com.nektony.*", "com.daisydiskapp.*", "org.sparkle-project.*",
];

/// Case-insensitive glob with `*` wildcards.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(pos) = rest.find(part) {
            rest = &rest[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

pub fn is_protected_name(name: &str) -> bool {
    PROTECTED_NAMES.iter().any(|p| glob_match(p, name))
}

/// Folders that hold personal data: nothing inside them is ever deleted, whatever a rule says,
/// minus the listed `NEVER` entries (home-relative, e.g. `"Movies"`; empty for the full list).
pub(crate) fn protected_path_except(p: &Path, allow: &[&str]) -> bool {
    const NEVER: &[&str] = &[
        "Library/Keychains", "Library/Mail", "Library/Accounts", "Library/Calendars", "Library/Contacts", "Library/Messages", "Library/Mobile Documents",
        "Library/Application Support/MobileSync", "Library/Application Support/AddressBook", "Library/Photos", "Library/Cookies", "Library/Preferences",
        "Library/Group Containers/group.com.apple.notes", "Library/CloudStorage", "Library/Application Support/FileProvider", ".ssh", ".gnupg", ".aws", ".kube",
        ".config/dusty", "Pictures", "Movies", "Music", "Desktop",
    ];
    NEVER.iter().filter(|n| !allow.contains(n)).any(|n| p.starts_with(h(n))) || p.to_string_lossy().to_lowercase().contains(".photoslibrary")
}
