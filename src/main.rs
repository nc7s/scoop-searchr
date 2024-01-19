use {
	std::{
		fs,
		path::{Path, PathBuf},
	},
	serde::Deserialize,
	anyhow::{Context, Result, bail},
};

#[derive(Deserialize, Debug)]
struct ScoopConfig {
	root_path: Option<PathBuf>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum ManifestBinItem {
	Path(PathBuf),
	Command(Vec<String>),
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum ManifestBinField {
	Path(PathBuf),
	PathOrCommandList(Vec<ManifestBinItem>),
}

#[derive(Deserialize, Debug)]
struct Manifest {
	version: String,
	bin: Option<ManifestBinField>,
	description: Option<String>,
}

fn scoop_home() -> Result<PathBuf> {
	if let Ok(env_var) = std::env::var("SCOOP") {
		let env_path = PathBuf::from(env_var);
		if !env_path.exists() {
			bail!("The SCOOP environment variable is set ({env_path:?}) but it does not exist");
		}
		Ok(env_path)
	} else {
		let user_home = directories::UserDirs::new()
			.context("can not locate user home directory")?
			.home_dir()
			.to_owned();
		let config_home = std::env::var("XDG_CONFIG_HOME")
			.map_or(user_home.join(".config"), PathBuf::from);
		let config_json = config_home.join("scoop").join("config.json");
		if let Ok(file) = fs::File::open(config_json) {
			if let Ok(ScoopConfig{ root_path: Some(root_path) }) = serde_json::from_reader(&file) {
				return Ok(root_path)
			}
		}
		let default = user_home.join("scoop");
		if default.exists() {
			Ok(default)
		} else {
			bail!("failed to fall back to default location, does not exist")
		}
	}
}

#[derive(PartialEq, PartialOrd, Eq, Ord)]
struct FindEntry {
	name: String,
	version: String,
	bin: Option<PathBuf>,
	description: Option<String>,
}

fn find_manifests(base: &Path, term: &str) -> Result<Vec<FindEntry>> {
	let term = term.to_lowercase();
	let walk = base.read_dir()
		.with_context(|| format!("failed to list manifests in {base:?}"))?;
	let mut results = Vec::new();

	for maybe_entry in walk {
		let path = match maybe_entry {
			Ok(entry) => entry.path(),
			Err(e) => {
				eprintln!("Error walking directory {base:?}: {e:?}");
				continue
			},
		};

		if path.extension().map(|ext| ext.to_str()) != Some(Some("json")) {
			continue
		}

		let manifest = match fs::read(&path) {
			Ok(content) => match serde_json::from_slice::<Manifest>(&content) {
				Ok(manifest) => manifest,
				Err(e) => {
					eprintln!("Failed to parse manifest at {path:?}: {e:?}");
					continue
				}
			},
			Err(e) => {
				eprintln!("Failed to read manifest at {path:?}: {e:?}");
				continue
			}
		};

		let name = path.file_stem().unwrap().to_string_lossy().into_owned();
		if name.contains(&term) {
			results.push(FindEntry {
				name,
				version: manifest.version,
				bin: None,
				description: None,
			});
			continue
		}

		if let Some(bin_field) = manifest.bin {
			let bins = match bin_field {
				ManifestBinField::Path(path) => vec![path],
				ManifestBinField::PathOrCommandList(list) => list
					.into_iter()
					.filter_map(|item| match item {
						ManifestBinItem::Command(command) => command.first().map(PathBuf::from),
						ManifestBinItem::Path(path) => Some(path),
					})
					.collect(),
			};
			if let Some(bin_path) = bins.into_iter().find(|bin| bin.file_stem()
				.unwrap()
				.to_string_lossy()
				.to_lowercase()
				.contains(&term)
			) {
				results.push(FindEntry {
					name,
					version: manifest.version,
					bin: Some(bin_path),
					description: None,
				});
				continue
			}
		}

		if let Some(description) = manifest.description {
			dbg!(&name, &description);
			if description.to_lowercase().contains(&term) {
				results.push(FindEntry {
					name,
					version: manifest.version,
					bin: None,
					description: Some(description),
				})
			}
		}
	}

	results.sort();
	Ok(results)
}

/* Copied from https://github.com/shilangyu/scoop-search/blob/8b6b1809cd5d8d03735d39bc5a16e9556328927d/args.go#L25 */
const HOOK: &str = r#"function scoop { if ($args[0] -eq "search") { scoop-searchr.exe @($args | Select-Object -Skip 1) } else { scoop.ps1 @args } }"#;

fn main() -> Result<()> {
	let arg = std::env::args().nth(1).unwrap_or("".to_string());
	if arg == "--hook" {
		println!("{}", HOOK);
		return Ok(())
	}
	let term = arg;
	let mut found = false;

	let scoop_home = scoop_home()?;
	if !scoop_home.exists() {
		eprintln!("Failed to find a valid scoop installation");
		std::process::exit(1);
	}

	let buckets_base = scoop_home.join("buckets");
	for base in buckets_base.read_dir()
		.with_context(|| format!("failed to list buckets directory: {buckets_base:?}"))?
	{
		let (bucket, path) = match base {
			Ok(base) => {
				let path = base.path();
				let name = path.file_name().unwrap().to_string_lossy().into_owned();
				let separate = path.join("bucket");
				(name, if separate.exists() {
					separate
				} else {
					path
				})
			},
			Err(e) => {
				eprintln!("Error listing bucket directory: {e:?}");
				continue
			}
		};

		let entries = find_manifests(&path, &term)?;
		if entries.is_empty() {
			continue;
		}
		found = true;

		println!("'{bucket}' bucket:");
		for FindEntry { name, version, bin, description } in entries {
			println!("	{name} ({version}){}{}", if let Some(bin) = bin {
				format!(" --> includes '{bin:?}'")
			} else { "".to_string() }, if let Some(description) = description {
				format!(": {description}")
			} else { "".to_string() });
		}
		println!();
	}

	if found {
		Ok(())
	} else {
		println!("No match found");
		std::process::exit(1)
	}
}
