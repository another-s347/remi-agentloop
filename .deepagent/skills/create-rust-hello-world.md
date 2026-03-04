# Create Rust Hello-World Project

This skill creates a minimal Rust "Hello, world!" project.

## Steps

1. **Create directory structure**
   ```bash
   mkdir -p /path/to/project/src
   ```

2. **Create Cargo.toml**
   ```toml
   [package]
   name = "project-name"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   ```

3. **Create src/main.rs**
   ```rust
   fn main() {
       println!("Hello, world!");
   }
   ```

4. **Build the project**
   ```bash
   cd /path/to/project && cargo build
   ```

## Verification

List the target directory to confirm build artifacts exist:
```bash
ls -la /path/to/project/target/debug/
```

## Run the executable

```bash
./target/debug/project-name
```
