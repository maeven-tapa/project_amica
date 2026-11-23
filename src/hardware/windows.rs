use serde::Deserialize;
use wmi::WMIConnection;

use super::Spec;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Computer {
    manufacturer: Option<String>,
    model: Option<String>,
    total_physical_memory: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OperatingSystem {
    caption: Option<String>,
    build_number: Option<String>,
    #[serde(rename = "OSArchitecture")]
    architecture: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Processor {
    name: Option<String>,
    number_of_cores: Option<u32>,
    number_of_logical_processors: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Graphics {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Drive {
    model: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Board {
    manufacturer: Option<String>,
    product: Option<String>,
}

pub(super) fn scan() -> Result<Vec<Spec>, &'static str> {
    let connection =
        WMIConnection::new().map_err(|_| "Amica could not access Windows hardware information.")?;
    let mut specs = Vec::new();

    for computer in query::<Computer>(
        &connection,
        "SELECT Manufacturer, Model, TotalPhysicalMemory FROM Win32_ComputerSystem",
    ) {
        add(
            &mut specs,
            "Computer",
            [computer.manufacturer, computer.model],
        );
        add(
            &mut specs,
            "RAM",
            [computer
                .total_physical_memory
                .map(|bytes| format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0))],
        );
    }
    for os in query::<OperatingSystem>(
        &connection,
        "SELECT Caption, BuildNumber, OSArchitecture FROM Win32_OperatingSystem",
    ) {
        add(
            &mut specs,
            "Operating system",
            [
                os.caption,
                os.architecture,
                os.build_number.map(|build| format!("build {build}")),
            ],
        );
    }
    for cpu in query::<Processor>(
        &connection,
        "SELECT Name, NumberOfCores, NumberOfLogicalProcessors FROM Win32_Processor",
    ) {
        add(
            &mut specs,
            "Processor",
            [
                cpu.name,
                cpu.number_of_cores.map(|cores| format!("{cores} cores")),
                cpu.number_of_logical_processors
                    .map(|threads| format!("{threads} logical processors")),
            ],
        );
    }
    for graphics in query::<Graphics>(&connection, "SELECT Name FROM Win32_VideoController") {
        add(&mut specs, "Graphics", [graphics.name]);
    }
    for drive in query::<Drive>(
        &connection,
        "SELECT Model, Size FROM Win32_DiskDrive WHERE InterfaceType <> 'USB'",
    ) {
        add(
            &mut specs,
            "Storage drive",
            [
                drive.model,
                drive.size.map(|bytes| {
                    if bytes >= 1_000_000_000_000 {
                        format!("{:.2} TB", bytes as f64 / 1_000_000_000_000.0)
                    } else {
                        format!("{:.0} GB", bytes as f64 / 1_000_000_000.0)
                    }
                }),
            ],
        );
    }
    for board in query::<Board>(
        &connection,
        "SELECT Manufacturer, Product FROM Win32_BaseBoard",
    ) {
        add(
            &mut specs,
            "Motherboard",
            [board.manufacturer, board.product],
        );
    }

    if specs.is_empty() {
        Err("Windows returned no system specs.")
    } else {
        Ok(specs)
    }
}

fn query<T: serde::de::DeserializeOwned>(connection: &WMIConnection, statement: &str) -> Vec<T> {
    connection.raw_query(statement).unwrap_or_default()
}

fn add<const N: usize>(specs: &mut Vec<Spec>, label: &'static str, parts: [Option<String>; N]) {
    let parts = parts
        .iter()
        .filter_map(Option::as_deref)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if !parts.is_empty() {
        specs.push(Spec {
            label,
            value: parts.join(" · "),
        });
    }
}
