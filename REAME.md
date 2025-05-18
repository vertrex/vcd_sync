# VCD Sync 

A command-line tool to merge and resynchronize VCD (Value Change Dump) files based on a common reset signal. This tool is designed to help engineers and developers working with digital circuits and hardware simulations to combine multiple VCD traces into a single, synchronized trace. 

## Features

- **Merge Multiple VCD Files**: Combine multiple VCD files into a single output file.
- **Resynchronize Traces**: Align traces based on a common reset signal.
- **Handle Duplicate Signals**: Automatically rename duplicate signals to avoid conflicts.
- **Flexible Command-Line Interface**: Easily specify input files, reset signal, and output file.

## Installation

### Prerequisites

- Rust (latest stable version)
- Cargo (Rust package manager)

### Building from Source

1. Clone the repository:

   ```sh
   git clone https://github.com/vertrex/vcd_sync.git
   cd vcd-sync
   ```

2. Build the project:

   ```sh
   cargo build --release
   ```

3. The compiled binary will be available in the `target/release` directory.

## Usage

### Command-Line Arguments

| Argument         | Description                                      | Required |
|------------------|--------------------------------------------------|----------|
| `vcd_files`      | Paths to the VCD files to merge                  | Yes      |
| `--reset_signal` | Name of the reset signal to resynchronize on     | Yes      |
| `--output_file`  | Path to the output merged VCD file               | Yes      |

### Examples

1. **Basic Usage**:

   ```sh
   ./vcd_sync file1.vcd file2.vcd --reset_signal reset --output_file merged.vcd
   ```

2. **Merging Multiple Files**:

   ```sh
   ./vcd_sync file1.vcd file2.vcd file3.vcd --reset_signal reset --output_file merged.vcd
   ```

### Detailed Steps

1. **Specify Input Files**: Provide the paths to the VCD files you want to merge.

2. **Specify Reset Signal**: Use the `--reset_signal` option to specify the name of the reset signal to resynchronize on (this signal name must be the same in every files).

3. **Specify Output File**: Use the `--output_file` option to specify the path to the output merged VCD file.

## Contributing

Contributions are welcome! Please fork the repository and submit a pull request with your changes. Ensure that your code adheres to the existing style and includes appropriate tests.

## License

This project is licensed under the GPLv3 License.

## Contact

For questions or feedback, please open an issue on the [GitHub repository](https://github.com/vertrex/vcd_sync).
