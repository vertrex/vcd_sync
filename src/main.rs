use std::fs::File;
use std::io::{BufReader};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser as ClapParser;

use vcd::Parser;
use vcd::{ScopeItem, IdCode};
use vcd::Command::ChangeScalar;
use vcd::Command::Timestamp;


/// A tool to merge and synchronize two VCD files based on a common reset signal.
#[derive(ClapParser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the first VCD file
    #[arg(short, long)]
    vcd_file1: PathBuf,

    /// Path to the second VCD file
    #[arg(short, long)]
    vcd_file2: PathBuf,

    /// Name of the reset signal to synchronize on
    #[arg(short, long)]
    sync_signal: String,

    /// Path to the output merged VCD file
    #[arg(short, long)]
    output_file: PathBuf,
}

fn open_vcd(file_path: &PathBuf) -> Parser<BufReader<File>> {
    let file = File::open(file_path).expect("Unable to open file");
    let file = BufReader::new(file);
    let parser = Parser::new(file);

    parser
}
/*
fn synchronize_and_merge(
    vcd_data1: HashMap<String, Vec<(u64, Value)>>,
    vcd_data2: HashMap<String, Vec<(u64, Value)>>,
    reset_signal_name: &str,
) -> HashMap<String, Vec<(u64, Value)>> {
    let reset_signal1 = vcd_data1.get(reset_signal_name).expect("Reset signal not found in first VCD");
    let reset_signal2 = vcd_data2.get(reset_signal_name).expect("Reset signal not found in second VCD");

    let time_offset = reset_signal2[0].0 - reset_signal1[0].0;

    let mut merged_data = vcd_data1;

    for (signal_name, signal_data) in vcd_data2 {
        let adjusted_signal_data = signal_data
            .into_iter()
            .map(|(time, value)| (time - time_offset, value))
            .collect();
        merged_data.insert(signal_name, adjusted_signal_data);
    }

    merged_data
}

fn write_vcd(merged_data: HashMap<String, Vec<(u64, Value)>>, output_file_path: &str) {
    let mut writer = VcdWriter::new(File::create(output_file_path).expect("Unable to create file"), Timescale::Ns).expect("Unable to create VCD writer");

    for (signal_name, signal_data) in merged_data {
        let var = Variable::new(
            signal_name.clone(),
            1,
            Variable::VarType::Wire,
            Scope::new("top", Scope::ScopeType::Module),
        );
        writer.add_var(&var).expect("Unable to add variable");

        for (time, value) in signal_data {
            writer.change_value(&var, time, value).expect("Unable to change value");
        }
    }

    writer.finish().expect("Unable to finish writing VCD");
}
*
*/

fn search_in_scopes(items: &[ScopeItem], sync_signal: &str) -> Result<Option<IdCode>> {
    for item in items {
        match item {
            ScopeItem::Var(var) if var.reference == sync_signal => {
                return Ok(Some(var.code));
            }
            ScopeItem::Scope(scope) => {
                if let Some(id_code) = search_in_scopes(&scope.items, sync_signal)? {
                    return Ok(Some(id_code));
                }
            }
            _ => (),
        }
    }
    Ok(None)
}

fn find_id_codes<T>(vcd_1: &mut Parser<T>, vcd_2: &mut Parser<T>, sync_signal: &str) -> Result<(IdCode, IdCode)>
where
    T: std::io::BufRead,
{
    let vcd_1_header = vcd_1.parse_header().context("Failed to parse header for VCD 1")?;
    let (tsv_1, timescale_1) = vcd_1_header.timescale.context("Timescale not found in VCD 1")?;
    let vcd_2_header = vcd_2.parse_header().context("Failed to parse header for VCD 2")?;
    let (tsv_2, timescale_2) = vcd_2_header.timescale.context("Timescale not found in VCD 2")?;

    if tsv_1 != tsv_2 {
        bail!("Error: Timescale values are different: {} {}", tsv_1, tsv_2);
    }

    if timescale_1 != timescale_2 {
        bail!("Error: Timescale units are different: {} {}", timescale_1, timescale_2);
    }

    let id_code_1 = search_in_scopes(&vcd_1_header.items, sync_signal)
        .context("Failed to search in scopes for VCD 1")?
        .ok_or_else(|| anyhow::anyhow!("Error: Sync signal not found in VCD 1"))?;

    let id_code_2 = search_in_scopes(&vcd_2_header.items, sync_signal)
        .context("Failed to search in scopes for VCD 2")?
        .ok_or_else(|| anyhow::anyhow!("Error: Sync signal not found in VCD 2"))?;

    Ok((id_code_1, id_code_2))
}


fn find_sync<T>(vcd : &mut Parser<T>, id_code : IdCode) -> u64
where
    T: std::io::BufRead,
{

    let mut current_timestamp = 0;
    let mut sync_timestamp = 0;

    for event in vcd
    {
        if let Ok(event) = event
        {
          match event
          {
            ChangeScalar(id, value) =>
            {
                //either we stop at fist 0 or fist 1
                //depending if logic low or high
                //or we stop at last 1 value of reset
                //meaning it will not change anymore
                //than mean it's stable and we can now sync trace ?
                //
                //Here reset is active low
                //so we wait for last reset == 1 value
                //because it mean reset is not active anymore
                //and get that timestamp to sync
                //and return it so we can sync the two traces
                if id == id_code && value == true.into() // && value == 0 if active high
                {
                    sync_timestamp = current_timestamp;
                    //println!("id {} value {}", id, value);
                    //break a first 1 ?
                    //or break at first 0 ?
                    //break;

                }
            },

            Timestamp(timestamp) =>
            {
              current_timestamp = timestamp;
            },
            _ => (),
          }
        }
    }

    sync_timestamp
}


fn main()  -> Result<()>
{
    let args = Args::parse();

    println!("parsing {}", &args.vcd_file1.display());
    let mut vcd_1 : Parser<_> = open_vcd(&args.vcd_file1);
    let mut vcd_2 : Parser<_> = open_vcd(&args.vcd_file2);

    let (id_code_1, id_code_2) = find_id_codes(&mut vcd_1, &mut vcd_2, &args.sync_signal)?;

    let sync_start_1 = find_sync(&mut vcd_1, id_code_1);
    let sync_start_2 = find_sync(&mut vcd_2, id_code_2);

    println!("first trace reset start at {}", sync_start_1);
    println!("second trace reset start at {}", sync_start_2);
    //let merged_data = synchronize_and_merge(vcd_data1, vcd_data2, sync_start_1, sync_start_2);
    //write_vcd(merged_data, "merged_trace.vcd");

    Ok(())
}
