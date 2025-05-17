use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, bail};
use clap::Parser as ClapParser;

use vcd::{Parser, Value, ScopeItem, IdCode, TimescaleUnit};
use vcd::Command::{ChangeScalar, Timestamp};

/// A tool to merge and resynchronize VCD files based on a common reset signal.
#[derive(ClapParser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the first VCD file
    #[arg(short, long)]
    vcd_file1: PathBuf, //XXX files : Vec<PathBuf>

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

type HeaderValues = Vec<(String, IdCode)>;
type Data = BTreeMap<u64, Vec<(u32, Value)>>;

pub struct VCD
{
    pub tsv : u32,
    pub timescale : TimescaleUnit,
    pub header : HeaderValues,
    pub data : Data,
    pub rst_end : u64,
    pub rst_id : IdCode,
}

impl VCD
{
    pub fn new(file_path : &PathBuf, reset_signal : &str) -> Result<VCD>
    {
        let mut parser = Parser::new(BufReader::new(File::open(file_path)?));

        let parsed_header = parser.parse_header()?;
        let (tsv, timescale) = parsed_header.timescale.context("Timescale not found in VCD 1")?;
        let header = collect_header(&parsed_header.items);
        let split = reset_signal.split(".").collect::<Vec<&str>>();
        let rst_id = parsed_header.find_var(&split).context("Reset signal not found in vcd 1")?.code;
        let (data, rst_end) = collect_data(&header, &mut parser, rst_id);
        println!("Reset signal end found at : {}", rst_end);

        Ok(VCD{ tsv, timescale, header, data, rst_id, rst_end })
    }

    pub fn merge(&mut self, vcd : VCD)
    {
        let timeskew = self.rst_end - vcd.rst_end;

        let header_id_start = self.header.len() as u32;
        // not working or name are merged ?
        self.header.extend(vcd.header.clone());
        //println!("HEADER MERGED {:?}", self.header);

        //XXX skip same reset of merged signal or rename it ?
        for (timestamp, data) in vcd.data.iter()
        {
            let synced = timestamp - timeskew;
            let entry = self.data.entry(synced).or_insert_with(Vec::new);
            for (id, value) in data
            {
                entry.push((*id + header_id_start, *value));
            }
        }
    }
}

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

fn write_vcd(merged : VCD, output_file : &PathBuf) -> Result<()>
{
    let mut writer = vcd::Writer::new(BufWriter::new(File::create(output_file)?));
    //USE REAL HeaderValues VALUE XXX
    writer.timescale(1, TimescaleUnit::NS)?;

    //create module ask name in entry ??
    writer.add_module("top")?;
    let mut header_map : HashMap<u32, IdCode>  =  HashMap::new();

    for (i, (header_name, _id_code)) in merged.header.into_iter().enumerate()
    {
        //XXX create module for each to keep structure ?
        //or for each file file1, file2 etc ?
        let header_name = header_name.split('.').last().unwrap_or(&header_name);
        //check to not create same name twice
        let id_code = writer.add_wire(1, header_name)?;
        header_map.insert(i as u32, id_code);
    }

    writer.upscope()?;
    writer.enddefinitions()?;

    for (timestamp, data) in merged.data.into_iter()
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

    println!("Parsing file : {}", args.vcd_file1.display());
    let mut vcd_1 = VCD::new(&args.vcd_file1, &args.reset_signal)?;
    println!("Parsing file : {}", args.vcd_file2.display());
    let mut vcd_2 = VCD::new(&args.vcd_file2, &args.reset_signal)?;

    if vcd_1.tsv != vcd_2.tsv
    {
        bail!("Error: Timescale values are different: {} {}", vcd_1.tsv, vcd_2.tsv);
    }

    if vcd_1.timescale != vcd_2.timescale
    {
        bail!("Error: Timescale units are different: {} {}", vcd_1.timescale, vcd_2.timescale);
    }

    println!("Resyncing and merging traces"); //show name ?
    let merged = match vcd_1.rst_end > vcd_2.rst_end
    {
      true => { vcd_1.merge(vcd_2); vcd_1 }
      false => { vcd_2.merge(vcd_1); vcd_2 }
    };

    println!("Writing merged trace in : {}", args.output_file.display());
    write_vcd(merged, &args.output_file)?;
    //XXX write FST directly ?
    //but if we write fst we can't remerge with an other file ...
    //but we can take multiple file as input and merge them all
    //and then write fst rather than launching the tool multiple time ...
    //write_fst(merged, &args.output_file)?;

    Ok(())
}
