use std::fs::File;
use std::path::PathBuf;
use std::io::{BufReader, BufWriter};
use std::collections::{BTreeMap, HashMap};

use clap::Parser as ClapParser;
use anyhow::{Context, Result, bail};
use vcd::Command::{ChangeScalar, Timestamp};
use vcd::{Parser, Value, ScopeItem, IdCode, TimescaleUnit};

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

// Signal name / Id code
type SignalsCode = Vec<(String, IdCode)>;
// Time stamp  : [Value Changed]
type TimestampValues = BTreeMap<u64, Vec<(u32, Value)>>;

pub struct VCD
{
    pub timescale_value: u32,
    pub timescale_unit : TimescaleUnit,
    pub signals : Vec<String>,
    pub values : TimestampValues,
    pub rst_end : u64,
    pub rst_id : IdCode,
}

impl VCD
{
    pub fn new(file_path : &PathBuf, reset_signal : &str) -> Result<VCD>
    {
        let mut parser = Parser::new(BufReader::new(File::open(file_path)?));

        let parsed_header = parser.parse_header()?;
        let (timescale_value, timescale_unit) = parsed_header.timescale.context("Timescale not found in VCD 1")?;
        let split = reset_signal.split(".").collect::<Vec<&str>>();
        let rst_id = parsed_header.find_var(&split).context("Reset signal not found in vcd 1")?.code;
        let signals_id = signals(&parsed_header.items);
        let (values, rst_end) = collect_values(&signals_id, &mut parser, rst_id);
        println!("Reset signal end found at : {}", rst_end);

        let signals : Vec<String> = signals_id.into_iter().map(|(sig_name, _sig_id)| (sig_name)).collect();
        Ok(VCD{ timescale_value, timescale_unit, signals, values, rst_id, rst_end })
    }

    pub fn merge(&mut self, vcd : VCD)
    {
        println!("Merging files with a timeskew of {} {}",
                 self.rst_end - vcd.rst_end,
                 self.timescale_unit);
        let timeskew = self.rst_end - vcd.rst_end;

        let signals_id_start = self.signals.len() as u32;
        //XXX we should remove all 'none' signals
        //created by acquisiton tool
        for vcd_signal in &vcd.signals
        {
            match self.signals.contains(vcd_signal)
            {
                true => self.signals.push(format!("{}_2", vcd_signal)),
                false => self.signals.push(vcd_signal.clone()),
            }
        }

        for (timestamp, values) in vcd.values.iter()
        {
            let synced = timestamp + timeskew;
            let entry = self.values.entry(synced).or_insert_with(Vec::new);
            for (id, value) in values
            {
                entry.push((*id + signals_id_start, *value));
            }
        }

        //we set everything at 0 at timestamp 0 to avoid Error
        //in gtkwave
        let mut init = Vec::new();
        for id in 0..self.signals.len()
        {
            // set it low by default ?
            init.push((id as u32, Value::V0));
        }
        self.values.insert(0, init);
    }
}

fn signals(items: &[ScopeItem]) -> SignalsCode {
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

fn collect_values<T>(signals: &SignalsCode, vcd: &mut Parser<T>, id_code : IdCode) -> (TimestampValues, u64)
where
    T: std::io::BufRead,
{
    let mut values: TimestampValues = TimestampValues::new();
    let mut current_timestamp = 0;
    let mut reset_timestamp = 0;

    let mut id_map : HashMap<IdCode, u32> = HashMap::new();

    for (i, (_, id_code)) in signals.iter().enumerate()
    {
        id_map.insert(*id_code, i as u32);
    }

    for cmd in vcd.into_iter().flatten()
    {
        match cmd
        {
            ChangeScalar(id, value) =>
            {
                values.entry(current_timestamp)
                    .or_insert_with(Vec::new)
                    .push((id_map[&id], value));
                //Here reset is active low
                //so we wait for last reset == 1 value
                //because it mean reset is not active anymore
                //then we get that timestamp to use it to sync
                //the traces
                if id == id_code && value == true.into()
                {
                    reset_timestamp = current_timestamp;
                }
            },
            Timestamp(timestamp) =>
            {
              current_timestamp = timestamp;
            },
            // XXX collect other value type ?
            _ => (),
        }
    }

    (values, reset_timestamp)
}

fn write_vcd(merged : VCD, output_file : &PathBuf) -> Result<()>
{
    let mut writer = vcd::Writer::new(BufWriter::new(File::create(output_file)?));
    writer.timescale(merged.timescale_value, merged.timescale_unit)?;

    //create module ask name in entry ??
    writer.add_module("top")?;

    //REMOVE ALL NONE SIGNALS created by acquisiton tool ?
    let mut signals_map : HashMap<u32, IdCode>  =  HashMap::new();
    for (i, signal_name) in merged.signals.into_iter().enumerate()
    {
        //XXX create module for each to keep structure ?
        //or for each file file1, file2 etc ?
        let signal_name = signal_name.split('.').last().unwrap_or(&signal_name);
        //XXX check to not create same name twice or did the lib do it ?
        let id_code = writer.add_wire(1, signal_name)?;
        signals_map.insert(i as u32, id_code);
    }

    writer.upscope()?;
    writer.enddefinitions()?;

    for (timestamp, values) in merged.values.into_iter()
    {
        writer.timestamp(timestamp)?;
        for (id, value) in values
        {
          let id_code = signals_map[&id];
          writer.change_scalar(id_code, value)?;
        }
    }

    Ok(())
}

fn main()  -> Result<()>
{
    let args = Args::parse();

    //XXX implement merge multiple files
    //let vcd_1 = files.take(1);
    //for file in files

    println!("Parsing file : {}", args.vcd_file1.display());
    let mut vcd_1 = VCD::new(&args.vcd_file1, &args.reset_signal)?;
    println!("Parsing file : {}", args.vcd_file2.display());
    let mut vcd_2 = VCD::new(&args.vcd_file2, &args.reset_signal)?;

    if vcd_1.timescale_value != vcd_2.timescale_value
    {
        bail!("Error: Timescale values are different: {} {}", vcd_1.timescale_value, vcd_2.timescale_value);
    }

    if vcd_1.timescale_unit != vcd_2.timescale_unit
    {
        bail!("Error: Timescale units are different: {} {}", vcd_1.timescale_unit, vcd_2.timescale_unit);
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
