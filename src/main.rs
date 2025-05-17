use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, bail};
use clap::Parser as ClapParser;

use vcd::{Parser, Value, ScopeItem, IdCode, TimescaleUnit};
use vcd::Command::{ChangeScalar, Timestamp};

/// A tool to merge and resethronize two VCD files based on a common reset signal.
#[derive(ClapParser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the first VCD file
    #[arg(short, long)]
    vcd_file1: PathBuf,

    /// Path to the second VCD file
    #[arg(short, long)]
    vcd_file2: PathBuf,

    /// Name of the reset signal to resethronize on
    #[arg(short, long)]
    reset_signal: String,

    /// Path to the output merged VCD file
    #[arg(short, long)]
    output_file: PathBuf,
}

fn open_vcd(file_path: &PathBuf) -> Parser<BufReader<File>> {
    let file = File::open(file_path).expect("Unable to open file");
    let file = BufReader::new(file);
    Parser::new(file)
}


type HeaderValues = Vec<(String, IdCode)>;

fn collect_header(items: &[ScopeItem]) -> HeaderValues {
    let mut results = Vec::new();

    fn recursive_collect(
        items: &[ScopeItem],
        current_scope: &str,
        results: &mut Vec<(String, IdCode)>,
    ) {
        for item in items {
            match item {
                ScopeItem::Var(var) => {
                    let full_reference = if current_scope.is_empty() {
                        var.reference.clone()
                    } else {
                        format!("{}.{}", current_scope, var.reference)
                    };
                    results.push((full_reference, var.code));
                }
                ScopeItem::Scope(scope) => {
                    let new_scope = if current_scope.is_empty() {
                        scope.identifier.clone()
                    } else {
                        format!("{}.{}", current_scope, scope.identifier)
                    };
                    recursive_collect(&scope.items, &new_scope, results);
                }
                _ => (),
            }
        }
    }

    recursive_collect(items, "", &mut results);
    results
}

fn search_in_scopes(items: &[ScopeItem], reset_signal: &str) -> Option<IdCode> {
    for item in items {
        match item {
            ScopeItem::Var(var) if var.reference == reset_signal => {
                return Some(var.code);
            }
            ScopeItem::Scope(scope) => {
                if let Some(id_code) = search_in_scopes(&scope.items, reset_signal) {
                    return Some(id_code);
                }
            }
            _ => (),
        }
    }
    None
}

fn find_id_codes<T>(vcd_1: &mut Parser<T>, vcd_2: &mut Parser<T>, reset_signal: &str) -> Result<(HeaderValues, IdCode, HeaderValues, IdCode)>
where
    T: std::io::BufRead,
{
    let vcd_1_header = vcd_1.parse_header()?;
    let (tsv_1, timescale_1) = vcd_1_header.timescale.context("Timescale not found in VCD 1")?;
    let vcd_2_header = vcd_2.parse_header()?;
    let (tsv_2, timescale_2) = vcd_2_header.timescale.context("Timescale not found in VCD 2")?;

    if tsv_1 != tsv_2
    {
        bail!("Error: Timescale values are different: {} {}", tsv_1, tsv_2);
    }

    if timescale_1 != timescale_2
    {
        bail!("Error: Timescale units are different: {} {}", timescale_1, timescale_2);
    }

    let header_1 = collect_header(&vcd_1_header.items);
    let header_2 = collect_header(&vcd_2_header.items);

    //XXX use that and split at .
    //vcd_1_header.find_var([]
    let id_code_1 = search_in_scopes(&vcd_1_header.items, reset_signal)
        .ok_or_else(|| anyhow::anyhow!("Error: Sync signal not found in VCD 1"))?;

    let id_code_2 = search_in_scopes(&vcd_2_header.items, reset_signal)
        .ok_or_else(|| anyhow::anyhow!("Error: Sync signal not found in VCD 2"))?;

    Ok((header_1, id_code_1, header_2, id_code_2))
}

pub type Data = BTreeMap<u64, Vec<(u32, Value)>>;


fn collect_data<T>(header : &HeaderValues, vcd: &mut Parser<T>, id_code : IdCode) -> (Data, u64)
where
    T: std::io::BufRead,
{
    let mut data : Data = Data::new();
    let mut current_timestamp = 0;
    let mut reset_timestamp = 0;

    let mut id_map : HashMap<IdCode, u32> = HashMap::new();

    for (i, (_, id_code)) in header.iter().enumerate()
    {
        id_map.insert(*id_code, i as u32);
    }

    for cmd in vcd.into_iter().flatten()
    {
        match cmd
        {
            ChangeScalar(id, value) =>
            {
                //we collect the data
                data.entry(current_timestamp)
                    .or_insert_with(Vec::new)
                    .push((id_map[&id], value));
                //either we stop at fist 0 or fist 1
                //depending if logic low or high
                //or we stop at last 1 value of reset
                //meaning it will not change anymore
                //than mean it's stable and we can now reset trace ?
                //
                //Here reset is active low
                //so we wait for last reset == 1 value
                //because it mean reset is not active anymore
                //and get that timestamp to reset
                //and return it so we can sync the two traces
                if id == id_code && value == true.into()
                {
                    reset_timestamp = current_timestamp;
                }
            },
            Timestamp(timestamp) =>
            {
              current_timestamp = timestamp;
            },
            _ => (),
        }
    }

    (data, reset_timestamp)
}

fn merge_data(mut header_1 : HeaderValues, data_1 : Data, header_2 : HeaderValues, data_2 : Data, timeskew : u64) -> (HeaderValues, Data)
{
   let mut merged = data_1;

   let header_id_start = header_1.len() as u32;
   header_1.extend(header_2.clone());

   for (timestamp, data) in data_2.iter()
   {
      let synced = timestamp - timeskew;
      let entry = merged.entry(synced).or_insert_with(Vec::new);
      for (id, value) in data
      {
         entry.push((*id + header_id_start, *value));
      }
   }


   (header_1, merged)
}

fn write_vcd(header_merged : HeaderValues, merged: Data, output_file : &PathBuf) -> Result<()>
{
    let mut writer = vcd::Writer::new(BufWriter::new(File::create(output_file)?));
    //USE REAL HeaderValues VALUE XXX
    writer.timescale(1, TimescaleUnit::NS)?;

    //create module ask name in entry ??
    writer.add_module("top")?;
    let mut header_map : HashMap<u32, IdCode>  =  HashMap::new();

    for (i, (header_name, _id_code)) in header_merged.into_iter().enumerate()
    {
        let id_code = writer.add_wire(1, &header_name)?;
        header_map.insert(i as u32, id_code);
    }

    writer.upscope()?;
    writer.enddefinitions()?;

    for (timestamp, data) in merged
    {
        writer.timestamp(timestamp)?;
        for (id, value) in data
        {
          let id_code = header_map[&id];
          writer.change_scalar(id_code, value)?;
        }
    }

    Ok(())
}

fn main()  -> Result<()>
{
    let args = Args::parse();

    println!("parsing vcd file 1 {}", &args.vcd_file1.display());
    let mut vcd_1 : Parser<_> = open_vcd(&args.vcd_file1);
    println!("parsing vcd file 2 {}", &args.vcd_file1.display());
    let mut vcd_2 : Parser<_> = open_vcd(&args.vcd_file2);

    println!("searching id codes");
    let (header_1, id_code_1, header_2, id_code_2) = find_id_codes(&mut vcd_1, &mut vcd_2, &args.reset_signal)?;

    println!("searching end of reset signal");
    let (data_1, rst_end_1) = collect_data(&header_1, &mut vcd_1, id_code_1);
    println!("searching end of reset signal");
    let (data_2, rst_end_2) = collect_data(&header_2, &mut vcd_2, id_code_2);

    println!("first trace reset end at {}", rst_end_1);
    println!("second trace reset end at {}", rst_end_2);

    println!("merge data and syncing trace");
    let (header_merged, data_merged) = match rst_end_1 > rst_end_2
    {
      //XXX PASSE MERGED SIGNAL NAME RESET
      //BECAUSE WE NEED TO REMOVE IT DON'T NEED TO HAVE IT MULTIPLE TIME IN THE MERGED TRACE !
      true => merge_data(header_1, data_1, header_2, data_2, rst_end_1 - rst_end_2),
      false => merge_data(header_2, data_2, header_1, data_1, rst_end_2 - rst_end_1),
    };

    println!("writing merged trace");
    write_vcd(header_merged, data_merged, &args.output_file)?;
    //XXX write FST directly ?
    //but if we write fst we can't remerge with an other file ...
    //but we can take multiple file as input and merge them all
    //and then write fst rather than launching the tool multiple time ...
    //write_fst(merged, &args.output_file)?;

    Ok(())
}
