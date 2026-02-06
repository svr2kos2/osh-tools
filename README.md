# OSH Tools

A remote exec toolchain with a Linux relay + CLI and a Windows daemon.

## Quick Install (Linux VPS)

```bash
curl -fsSL https://raw.githubusercontent.com/svr2kos2/osh-tools/master/install.sh -o install.sh
sudo bash install.sh
```

What it does:
- Downloads the latest Linux release asset
- Installs `osh-relay`, `osh`, `osh-admin` to `/usr/local/bin`
- Creates `/etc/osh/devices.json` with a generated `secret_key`
- Registers and starts the `osh-relay` systemd service

## Directory Structure

```
osh/
├── SKILL.md                    # OpenClaw Skill definition
├── Cargo.toml                  # Rust workspace configuration
├── build-linux.sh              # Build script for Linux (VPS)
├── build-windows.bat           # Build script for Windows (Client)
├── bin/
│   ├── linux/                  # Linux binaries (build on VPS)
│   │   ├── osh-relay           # Relay server
│   │   ├── osh                 # CLI tool
│   │   └── osh-admin           # Admin tool
│   └── windows/
│       └── osh-daemon.exe      # Windows client daemon ✓
├── config/
│   ├── devices.json.example    # Server config template
│   └── daemon.toml.example     # Client config template
├── osh-relay/                  # Rust source: Relay server
├── osh-cli/                    # Rust source: CLI tools
└── osh-daemon/                 # Rust source: Client daemon
```

## Build Instructions

### On VPS (Linux/OpenCloudOS)

```bash
# 1. Upload the entire 'osh' folder to the VPS
scp -r osh/ user@vps:/path/to/

# 2. SSH into VPS
ssh user@vps

# 3. Install Rust if not installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 4. Build
cd /path/to/osh
chmod +x build-linux.sh
./build-linux.sh

# 5. Install binaries
sudo cp target/release/osh-relay /usr/local/bin/
sudo cp target/release/osh /usr/local/bin/
sudo cp target/release/osh-admin /usr/local/bin/

# 6. Setup configuration
sudo mkdir -p /etc/osh
sudo cp config/devices.json.example /etc/osh/devices.json
sudo vim /etc/osh/devices.json  # Set your secret_key

# 7. Run relay service
osh-relay --config /etc/osh/devices.json
```

### On Windows Client

The Windows daemon is pre-built at `bin/windows/osh-daemon.exe`.

```powershell
# 1. Copy osh-daemon.exe to your preferred location
Copy-Item bin\windows\osh-daemon.exe C:\Tools\

# 2. Initialize configuration
cd C:\Tools
.\osh-daemon.exe init --server wss://your-vps.com/bridge --secret your-secret-key

# 3. Run daemon
.\osh-daemon.exe run
```

## Systemd Service (VPS)

Create `/etc/systemd/system/osh-relay.service`:

```ini
[Unit]
Description=OSH Relay Service
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/osh-relay --config /etc/osh/devices.json
Restart=always
User=osh

[Install]
WantedBy=multi-user.target
```

Then:
```bash
sudo systemctl daemon-reload
sudo systemctl enable osh-relay
sudo systemctl start osh-relay
```

## Quick Test

```bash
# On VPS: List connected devices
osh-admin list

# On VPS: Approve a device
osh-admin approve <device_id>
osh-admin reload

# On VPS: Execute command on remote device
osh mypc "Get-Process | Select-Object -First 5"
```

## Releases

Releases are automatically created via GitHub Actions when a new tag is pushed. For information about:
- How to create releases
- Troubleshooting 403 permission errors
- Required repository configuration

See [RELEASE.md](RELEASE.md) for detailed instructions.
## 📊 Logging and Debugging

OSH Daemon now includes a comprehensive logging system for debugging and troubleshooting:

### Quick Start
- **New User?** Start with [QUICK_START_LOGGING.md](QUICK_START_LOGGING.md) (5 min)
- **Windows User?** Use [WINDOWS_QUICK_REFERENCE.md](WINDOWS_QUICK_REFERENCE.md)
- **Need Help?** See [DOCS_INDEX.md](DOCS_INDEX.md)

### Key Features
- ✅ File-based logging for daemon and GUI
- ✅ Automatic log rotation (daily)
- ✅ Complete connection state tracking
- ✅ Reconnection attempt logging
- ✅ IPC (named pipe) communication visibility
- ✅ Structured debug output

### Enable Logging
```bash
# Daemon with logging
osh-daemon run --log

# View logs
tail -f logs/daemon.log
tail -f logs/gui.log
```

### Documentation
| Document | Purpose | Time |
|----------|---------|------|
| [QUICK_START_LOGGING.md](QUICK_START_LOGGING.md) | Quick start guide | 5 min |
| [LOGGING_GUIDE.md](LOGGING_GUIDE.md) | Complete reference | 20 min |
| [WINDOWS_QUICK_REFERENCE.md](WINDOWS_QUICK_REFERENCE.md) | Windows command cheat sheet | Quick |
| [IMPLEMENTATION_SUMMARY.md](IMPLEMENTATION_SUMMARY.md) | Technical details | 15 min |
| [TEST_LOGGING.md](TEST_LOGGING.md) | Testing guide | 10 min |
| [VERIFICATION_CHECKLIST.md](VERIFICATION_CHECKLIST.md) | Verification checklist | 10 min |
| [FINAL_SUMMARY.md](FINAL_SUMMARY.md) | Complete overview | 15 min |
| [DOCS_INDEX.md](DOCS_INDEX.md) | Document index | Quick |