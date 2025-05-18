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
    /// Path to the VCD files
    #[arg(num_args = 2..)]
    vcd_files: Vec<PathBuf>,

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
        //Signal are already splitted
        //let signal_name = signal_name.split('.').next_back().unwrap_or(&signal_name);
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

        // Handle duplicate signals by appending "_2" to the signal name
        for vcd_signal in &vcd.signals
        {
            match self.signals.contains(vcd_signal)
            {
                true => self.signals.push(format!("{}_2", vcd_signal)),
                false => self.signals.push(vcd_signal.clone()),
            }
        }

        // Adjust timestamps and merge values
        for (timestamp, values) in vcd.values.iter()
        {
            let synced = timestamp + timeskew;
            let entry = self.values.entry(synced).or_default();
            for (id, value) in values
            {
                entry.push((*id + signals_id_start, *value));
            }
        }

        // Initialize all signals to 0 at timestamp 0 to avoid errors in GTKWavee
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
                        //format!("{}.{}", current_scope, var.reference)
                        var.reference.clone()
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
                    .or_default()
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

    //XXX get top module as arg ?
    //use file name so it's easier to know where signal came from ?
    writer.add_module("top")?;

    let mut signals_map : HashMap<u32, IdCode>  =  HashMap::new();
    for (i, signal_name) in merged.signals.into_iter().enumerate()
    {
        signals_map.insert(i as u32,  writer.add_wire(1, &signal_name)?);
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

    let mut vcd_files = args.vcd_files.iter();
    let main_vcd_file = vcd_files.next().unwrap();
    println!("Parsing file : {}", main_vcd_file.display());
    let mut main_vcd = VCD::new(main_vcd_file, &args.reset_signal)?;

    for current_vcd_file in vcd_files
    {
        println!("Parsing file : {}", current_vcd_file.display());
        let mut current_vcd = VCD::new(current_vcd_file, &args.reset_signal)?;

        if main_vcd.timescale_value != current_vcd.timescale_value
        {
            bail!("Error: Timescale values are different: {} {}",
                  main_vcd.timescale_value,
                  current_vcd.timescale_value);
        }

        if main_vcd.timescale_unit != current_vcd.timescale_unit
        {
            bail!("Error: Timescale units are different: {} {}",
                  main_vcd.timescale_unit,
                  current_vcd.timescale_unit);
        }

        println!("Resyncing and merging traces");
        main_vcd = match main_vcd.rst_end > current_vcd.rst_end
        {
          true =>  { main_vcd.merge(current_vcd); main_vcd }
          false => { current_vcd.merge(main_vcd); current_vcd }
        };
    }

    println!("Writing merged trace in : {}", args.output_file.display());
    write_vcd(main_vcd, &args.output_file)?;
    //XXX add features to write FST directly via fstlib ?
    //write_fst(merged, &args.output_file)?;

    Ok(())
}
