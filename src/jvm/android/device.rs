// Devices: apps installed and started with adb, instrumented tests run on them, on an emulator when no device is connected
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::common::system::{self, HostArch};
use crate::common::xml::{self, XmlElement};
use crate::language::runtime::{TestCounts, TestOutcome, TestRunner};

use super::sdk::{AndroidSdk, SDK_REPOSITORY, SdkCatalog};

/// catalogs of system images: directory below sys-img/ and how it is shown.
/// "android" holds the plain open source Android (AOSP).
const IMAGE_SOURCES: &[(&str, &str)] = &[
    ("android", "Android without Google services"),
    ("google_apis", "Google services without Play Store"),
    ("google_apis_playstore", "Google services with Play Store"),
];

/// newest images offered per source
const IMAGES_PER_SOURCE: usize = 3;
const BOOT_TIMEOUT: Duration = Duration::from_secs(600);

/// Installs the apk with adb and starts its launcher activity. Without a
/// connected device the user is guided to an emulator.
pub fn install_and_start(
    apk: &Path,
    sdk_root: &Path,
    application_id: &str,
    activity: Option<&str>,
    min_sdk: i64,
) -> Result<(), String> {
    let Some(activity) = activity else {
        return Err(format!(
            "{application_id} has no activity with a MAIN/LAUNCHER intent filter"
        ));
    };

    let adb = AndroidSdk::adb(sdk_root)?;
    let device = target_device(&adb, sdk_root, min_sdk)?;
    println!("Installing {} on {device}", apk.display());
    let mut install = Command::new(&adb);
    install
        .args(["-s", &device, "install", "-r", "-t"])
        .arg(apk);
    system::run(&mut install, "adb install")?;

    println!("Starting {application_id}/{activity}");
    let mut start = Command::new(&adb);
    start.args([
        "-s",
        &device,
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        &format!("{application_id}/{activity}"),
    ]);

    return system::run(&mut start, "adb shell am start");
}

/// The device adb works with: the one ANDROID_SERIAL names, the only one connected,
/// or an emulator when none is
fn target_device(adb: &Path, sdk_root: &Path, min_sdk: i64) -> Result<String, String> {
    let attached = adb_devices(adb)?;

    if let Some((serial, _)) = attached.iter().find(|(_, state)| state == "unauthorized") {
        return Err(format!(
            "{serial} does not allow USB debugging yet, confirm the prompt on the device and run again"
        ));
    }

    let devices: Vec<String> = attached
        .into_iter()
        .filter(|(_, state)| state == "device")
        .map(|(serial, _)| serial)
        .collect();

    // adb picks the device from ANDROID_SERIAL itself
    let serial = std::env::var("ANDROID_SERIAL").ok();
    match (devices.as_slice(), serial) {
        ([], _) => return start_or_set_up_emulator(adb, sdk_root, min_sdk),
        (_, Some(serial)) => return Ok(serial),
        ([device], None) => return Ok(device.clone()),
        (_, None) => {
            return Err(format!(
                "several devices are connected ({}), choose one with ANDROID_SERIAL",
                devices.join(", ")
            ));
        }
    }
}

/// When no device is connected: an emulator build-script set up, or a new one
/// the user sets up here. Returns its serial once it booted.
fn start_or_set_up_emulator(adb: &Path, sdk_root: &Path, min_sdk: i64) -> Result<String, String> {
    if !io::stdin().is_terminal() {
        return Err("no Android device or emulator is connected (see adb devices), run in a terminal to set up an emulator".to_string());
    }

    println!("\nNo Android device or emulator is connected.");
    // AVDs live in the build home instead of ~/.android/avd
    let home = system::data_dir("android/avd");
    let avds = list_avds(&home);

    let name = if avds.is_empty() {
        set_up_avd(sdk_root, &home, min_sdk)?
    } else {
        println!("\nEmulators set up by build-script:");
        for (index, avd) in avds.iter().enumerate() {
            println!("  {}) {}", index + 1, avd.display);
        }

        println!("  n) Set up a new emulator");

        match system::prompt("Choose [1]: ")?.as_str() {
            "" => avds[0].name.clone(),
            "n" => set_up_avd(sdk_root, &home, min_sdk)?,
            answer => {
                let index: usize = answer.parse().map_err(|_| "cancelled".to_string())?;
                avds.get(index.wrapping_sub(1))
                    .ok_or_else(|| "cancelled".to_string())?
                    .name
                    .clone()
            }
        }
    };

    let emulator = sdk_root
        .join("emulator")
        .join(format!("emulator{}", std::env::consts::EXE_SUFFIX));

    if !emulator.is_file() {
        SdkCatalog::load()?.install(sdk_root, "emulator", &sdk_root.join("emulator"), false)?;
    }

    return start_emulator(adb, &emulator, sdk_root, &home, &name);
}

/// Lets the user pick a system image, installs it with the emulator and creates the AVD
fn set_up_avd(sdk_root: &Path, home: &Path, min_sdk: i64) -> Result<String, String> {
    let abi = host_abi();
    println!("Loading the list of system images ...");

    let mut catalogs = vec![];

    for (source, _) in IMAGE_SOURCES {
        catalogs.push(SdkCatalog::load_from(&format!(
            "{SDK_REPOSITORY}/sys-img/{source}/sys-img2-3.xml"
        ))?);
    }

    let mut images = vec![];

    for (index, catalog) in catalogs.iter().enumerate() {
        for image in system_images(catalog, abi, min_sdk)
            .into_iter()
            .take(IMAGES_PER_SOURCE)
        {
            images.push((index, image));
        }
    }

    if images.is_empty() {
        return Err(format!(
            "there is no system image for {abi} with API {min_sdk} or newer"
        ));
    }

    println!("\nSystem images for {abi} (the app needs API {min_sdk} or newer):");
    let source_width = IMAGE_SOURCES
        .iter()
        .map(|(_, label)| label.len())
        .max()
        .unwrap_or(0);

    for (number, (catalog_index, image)) in images.iter().enumerate() {
        let source = IMAGE_SOURCES[*catalog_index].1;
        println!(
            "  {}) API {:<6} {:<source_width$} {}",
            number + 1,
            image.version,
            source,
            system::format_size(image.size)
        );
    }

    let answer = system::prompt("Choose [1]: ")?;
    let number: usize = if answer.is_empty() {
        1
    } else {
        answer.parse().map_err(|_| "cancelled".to_string())?
    };

    let (catalog_index, mut image) = images
        .get(number.wrapping_sub(1))
        .ok_or_else(|| "cancelled".to_string())?
        .clone();

    image.tag_display = IMAGE_SOURCES[catalog_index].1.to_string();

    let main_catalog = SdkCatalog::load()?;
    let emulator_dir = sdk_root.join("emulator");
    let image_dir = sdk_root.join(image.directory());

    let emulator_size = if emulator_dir.is_dir() {
        0
    } else {
        main_catalog.size("emulator").unwrap_or(0)
    };

    let image_size = if image_dir.is_dir() { 0 } else { image.size };

    if emulator_size + image_size > 0 {
        println!(
            "\nThis downloads {} to {}",
            system::format_size(emulator_size + image_size),
            sdk_root.display()
        );
        if !matches!(
            system::prompt("Continue? (Y/n): ")?.as_str(),
            "" | "y" | "yes"
        ) {
            return Err("cancelled".to_string());
        }
    }

    if !emulator_dir.is_dir() {
        main_catalog.install(sdk_root, "emulator", &emulator_dir, false)?;
    }

    if !image_dir.is_dir() {
        catalogs[catalog_index].install(sdk_root, &image.path, &image_dir, false)?;
    }

    let name = create_avd(home, &image)?;
    println!("Created emulator {name}");

    return Ok(name);
}

/// Starts the emulator in the background, it keeps running after build-script
/// ends. Waits until Android booted.
fn start_emulator(
    adb: &Path,
    emulator: &Path,
    sdk_root: &Path,
    home: &Path,
    name: &str,
) -> Result<String, String> {
    // the console port decides the serial: emulator-<port>
    let used: Vec<String> = adb_devices(adb)?
        .into_iter()
        .map(|(serial, _)| serial)
        .collect();

    let port = (5554..=5584)
        .step_by(2)
        .find(|port| !used.contains(&format!("emulator-{port}")))
        .ok_or("all emulator ports are in use")?;

    let serial = format!("emulator-{port}");

    let log_path = home.join(format!("{name}.log"));

    let log = fs::File::create(&log_path)
        .map_err(|error| format!("could not create {}: {error}", log_path.display()))?;

    let log_err = log.try_clone().map_err(|error| error.to_string())?;

    let mut command = Command::new(emulator);
    command
        .arg("-avd")
        .arg(name)
        .arg("-port")
        .arg(port.to_string())
        .args(["-no-snapshot-save", "-no-boot-anim"])
        // e.g. -no-window for machines without a display
        .args(
            std::env::var("BUILD_SCRIPT_EMULATOR_ARGS")
                .unwrap_or_default()
                .split_whitespace(),
        )
        .env("ANDROID_SDK_ROOT", sdk_root)
        .env("ANDROID_HOME", sdk_root)
        .env("ANDROID_AVD_HOME", home)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);

    // its own process group, so Ctrl+C in build-script does not stop it
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", emulator.display()))?;

    println!(
        "Starting emulator {name} as {serial} (log: {})",
        log_path.display()
    );

    let started = Instant::now();
    let mut reported = 0;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "the emulator stopped ({status}), see {}",
                log_path.display()
            ));
        }

        let booted = Command::new(adb)
            .args(["-s", &serial, "shell", "getprop", "sys.boot_completed"])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "1");

        if booted {
            println!("{serial} booted after {}s", started.elapsed().as_secs());
            return Ok(serial);
        }

        let elapsed = started.elapsed();

        if elapsed > BOOT_TIMEOUT {
            return Err(format!(
                "{serial} did not boot within {}s, see {}",
                BOOT_TIMEOUT.as_secs(),
                log_path.display()
            ));
        }

        if elapsed.as_secs() / 15 > reported {
            reported = elapsed.as_secs() / 15;
            println!("Waiting for {serial} to boot ({}s)", elapsed.as_secs());
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Serial and state of every device adb knows: device, offline, unauthorized
fn adb_devices(adb: &Path) -> Result<Vec<(String, String)>, String> {
    let output = Command::new(adb)
        .arg("devices")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start {}: {error}", adb.display()))?;

    return Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once('\t'))
        .map(|(serial, state)| (serial.to_string(), state.trim().to_string()))
        .collect());
}

/// Stable images of a catalog for the ABI with an API level of at least
/// `min_sdk`, newest first. Previews and 16 KB page size variants are left out.
fn system_images(catalog: &SdkCatalog, abi: &str, min_sdk: i64) -> Vec<SystemImage> {
    let mut images: Vec<SystemImage> = catalog
        .packages
        .iter()
        .filter_map(|remote| {
            let parts: Vec<&str> = remote.path.split(';').collect();

            if parts.len() != 4 || parts[0] != "system-images" || parts[3] != abi {
                return None;
            }

            let preview =
                parts[1].contains("-beta") || parts[2].ends_with("_ps16k") || remote.obsolete;

            if !remote.is_stable() || preview {
                return None;
            }

            // newer images write 37.0, older ones 36
            let api: i64 = remote
                .details
                .api_level
                .as_deref()?
                .trim()
                .split('.')
                .next()?
                .parse()
                .ok()?;

            if api < min_sdk {
                return None;
            }

            let size = remote.archive()?.complete.as_ref()?.size;

            let tag_display = remote
                .details
                .tags
                .first()
                .and_then(|tag| tag.display.as_deref())
                .map(str::trim);

            return Some(SystemImage {
                path: remote.path.clone(),
                api,
                version: parts[1].trim_start_matches("android-").to_string(),
                tag: parts[2].to_string(),
                tag_display: tag_display
                    .filter(|display| !display.is_empty())
                    .unwrap_or(parts[2])
                    .to_string(),
                abi: abi.to_string(),
                size,
            });
        })
        .collect();

    images.sort_by(|left, right| {
        right
            .api
            .cmp(&left.api)
            .then(right.version.cmp(&left.version))
    });

    return images;
}

/// The AVDs build-script set up
fn list_avds(home: &Path) -> Vec<Avd> {
    let Ok(entries) = fs::read_dir(home) else {
        return vec![];
    };

    let mut avds: Vec<Avd> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ini"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().to_string();

            let config = fs::read_to_string(home.join(format!("{name}.avd/config.ini"))).ok()?;
            let display = config
                .lines()
                .find_map(|line| line.strip_prefix("avd.ini.displayname="))
                .unwrap_or(&name)
                .to_string();

            return Some(Avd { name, display });
        })
        .collect();

    avds.sort_by(|left, right| left.name.cmp(&right.name));

    return avds;
}

/// Writes the files avdmanager would write, returns the AVD name
fn create_avd(home: &Path, image: &SystemImage) -> Result<String, String> {
    let name = format!(
        "build-script-api{}-{}",
        image.version.replace('.', "_"),
        image.tag
    );

    let avd_dir = home.join(format!("{name}.avd"));

    let cpu = if image.abi == "arm64-v8a" {
        "arm64"
    } else {
        "x86_64"
    };

    let play_store = image.tag.contains("playstore");

    let config = [
        format!("AvdId={name}"),
        format!(
            "avd.ini.displayname=Android API {} ({}, {})",
            image.version, image.tag_display, image.abi
        ),
        "avd.ini.encoding=UTF-8".to_string(),
        format!("abi.type={}", image.abi),
        format!("hw.cpu.arch={cpu}"),
        format!("image.sysdir.1={}/", image.directory()),
        format!("tag.id={}", image.tag),
        format!("tag.display={}", image.tag_display),
        format!("target=android-{}", image.api),
        format!("PlayStore.enabled={play_store}"),
        // a phone like a Pixel
        "hw.lcd.width=1080".to_string(),
        "hw.lcd.height=2400".to_string(),
        "hw.lcd.density=420".to_string(),
        "hw.ramSize=2048".to_string(),
        "hw.keyboard=yes".to_string(),
        "hw.gpu.enabled=yes".to_string(),
        "hw.gpu.mode=auto".to_string(),
        "disk.dataPartition.size=6442450944".to_string(),
        "showDeviceFrame=no".to_string(),
    ];

    system::write_file(&avd_dir.join("config.ini"), &(config.join("\n") + "\n"))?;

    let ini = format!(
        "avd.ini.encoding=UTF-8\npath={}\npath.rel=avd/{name}.avd\ntarget=android-{}\n",
        avd_dir.display(),
        image.api,
    );

    system::write_file(&home.join(format!("{name}.ini")), &ini)?;

    return Ok(name);
}

/// The ABI system images need to run fast on this machine
fn host_abi() -> &'static str {
    match system::host_arch() {
        Ok(HostArch::Aarch64) => return "arm64-v8a",
        _ => return "x86_64",
    }
}

/// A system image the emulator can boot
#[derive(Debug, Clone, PartialEq)]
struct SystemImage {
    /// system-images;android-36;default;arm64-v8a
    path: String,
    api: i64,
    /// as in the path: 36 or 37.0
    version: String,
    tag: String,
    tag_display: String,
    abi: String,
    size: u64,
}

impl SystemImage {
    /// Where the image is installed in the SDK, relative to its root
    fn directory(&self) -> String {
        return self.path.replace(';', "/");
    }
}

/// An emulator (AVD) set up by build-script
#[derive(Debug, PartialEq)]
struct Avd {
    name: String,
    display: String,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::jvm::android::sdk::tests::catalog;

    use super::*;

    fn image_package(path: &str, channel: &str, api: &str, size: &str) -> String {
        return format!(
            r#"<remotePackage path="{path}"><channelRef ref="{channel}"/>
            <type-details><api-level>{api}</api-level><tag><id>default</id><display>AOSP</display></tag></type-details>
            <archives><archive><complete><size>{size}</size><url>image.zip</url></complete></archive></archives>
        </remotePackage>"#
        );
    }

    #[test]
    fn emulator_images_are_filtered_for_the_host() {
        let packages = [
            image_package(
                "system-images;android-36;default;arm64-v8a",
                "channel-0",
                "36",
                "810000000",
            ),
            image_package(
                "system-images;android-35;default;arm64-v8a",
                "channel-0",
                "35",
                "769000000",
            ),
            image_package(
                "system-images;android-37.0;default;arm64-v8a",
                "channel-0",
                "37.0",
                "900000000",
            ),
            image_package(
                "system-images;android-23;default;arm64-v8a",
                "channel-0",
                "23",
                "400000000",
            ),
            image_package(
                "system-images;android-37;default;arm64-v8a",
                "channel-2",
                "37",
                "900000000",
            ),
            image_package(
                "system-images;android-37.2-beta1;default;arm64-v8a",
                "channel-0",
                "37",
                "900000000",
            ),
            image_package(
                "system-images;android-36;google_apis_ps16k;arm64-v8a",
                "channel-0",
                "36",
                "900000000",
            ),
            image_package(
                "system-images;android-36;default;x86_64",
                "channel-0",
                "36",
                "800000000",
            ),
        ]
        .concat();

        let images = system_images(&catalog(&packages), "arm64-v8a", 24);

        let found: Vec<(&str, i64, &str)> = images
            .iter()
            .map(|image| (image.path.as_str(), image.api, image.tag_display.as_str()))
            .collect();

        assert_eq!(
            found,
            [
                ("system-images;android-37.0;default;arm64-v8a", 37, "AOSP"),
                ("system-images;android-36;default;arm64-v8a", 36, "AOSP"),
                ("system-images;android-35;default;arm64-v8a", 35, "AOSP"),
            ]
        );
        assert_eq!(
            images[1].directory(),
            "system-images/android-36/default/arm64-v8a"
        );
        assert_eq!(system::format_size(images[1].size), "810 MB");
        assert_eq!(system::format_size(2_194_000_000), "2.2 GB");
    }

    #[test]
    fn avds_are_written_and_listed() {
        let home = std::env::temp_dir().join(format!("build-script-avd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);

        let image = SystemImage {
            path: "system-images;android-36;google_apis_playstore;arm64-v8a".to_string(),
            api: 36,
            version: "36".to_string(),
            tag: "google_apis_playstore".to_string(),
            tag_display: "Google Play".to_string(),
            abi: "arm64-v8a".to_string(),
            size: 1,
        };

        let name = create_avd(&home, &image).unwrap();
        assert_eq!(name, "build-script-api36-google_apis_playstore");

        let config = fs::read_to_string(home.join(format!("{name}.avd/config.ini"))).unwrap();

        for line in [
            "image.sysdir.1=system-images/android-36/google_apis_playstore/arm64-v8a/",
            "hw.cpu.arch=arm64",
            "PlayStore.enabled=true",
        ] {
            assert!(
                config.lines().any(|existing| existing == line),
                "{line} missing in {config}"
            );
        }

        assert!(
            fs::read_to_string(home.join(format!("{name}.ini")))
                .unwrap()
                .contains("target=android-36")
        );

        let avds = list_avds(&home);
        assert_eq!(
            avds,
            [Avd {
                name,
                display: "Android API 36 (Google Play, arm64-v8a)".to_string()
            }]
        );
        fs::remove_dir_all(&home).unwrap();
    }
}
