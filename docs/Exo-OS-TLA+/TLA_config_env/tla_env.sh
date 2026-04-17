#!/bin/bash
# TLA+ Environment Setup and Test Runner for Exo-OS
# Usage: source tla_env.sh [testing|install|check]
# Options:
#   testing   - Run environment checks
#   install   - Install all missing dependencies
#   check     - Alias for testing
#   setup     - Full installation and setup

set -o pipefail

# Detect and setup JAVA_HOME dynamically
detect_java_home() {
    if [[ -n "${JAVA_HOME}" ]] && [[ -d "${JAVA_HOME}" ]]; then
        return 0
    fi
    
    # Try common locations
    for java_path in \
        $(which java 2>/dev/null | xargs dirname | xargs dirname 2>/dev/null) \
        /usr/lib/jvm/java-11-openjdk \
        /usr/lib/jvm/java \
        /usr/local/openjdk-11 \
        /opt/java; do
        if [[ -f "${java_path}/bin/java" ]]; then
            echo "${java_path}"
            return 0
        fi
    done
    
    return 1
}

export JAVA_HOME="$(detect_java_home 2>/dev/null || echo '/usr/lib/jvm/java-11-openjdk')"
export PATH="/opt/tlaplus:${JAVA_HOME}/bin:${PATH}"
export TLATOOLS="/opt/tlaplus/tla2tools.jar"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[TLA+ Setup]${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_info() {
    echo -e "${MAGENTA}ℹ${NC} $1"
}

# Detect OS and package manager
detect_package_manager() {
    if command -v apk &> /dev/null; then
        echo "apk"
    elif command -v apt-get &> /dev/null; then
        echo "apt"
    elif command -v yum &> /dev/null; then
        echo "yum"
    elif command -v brew &> /dev/null; then
        echo "brew"
    else
        echo "unknown"
    fi
}

# Install packages based on package manager
install_packages() {
    local packages=$@
    local pm=$(detect_package_manager)
    
    if [[ "$pm" == "unknown" ]]; then
        print_error "Could not detect package manager. Please install packages manually: $packages"
        return 1
    fi
    
    print_status "Installing packages using $pm: $packages"
    
    case "$pm" in
        apk)
            apk update && apk add --no-cache $packages
            ;;
        apt)
            apt-get update && apt-get install -y $packages
            ;;
        yum)
            yum install -y $packages
            ;;
        brew)
            brew install $packages
            ;;
    esac
    
    if [[ $? -eq 0 ]]; then
        print_success "Packages installed successfully"
        return 0
    else
        print_error "Failed to install packages"
        return 1
    fi
}

# Install Java if not present
install_java() {
    if command -v java &> /dev/null; then
        print_success "Java already installed"
        # Update JAVA_HOME after finding java
        export JAVA_HOME="$(detect_java_home 2>/dev/null || echo '/usr/lib/jvm/java-11-openjdk')"
        return 0
    fi
    
    print_status "Java not found. Installing..."
    local pm=$(detect_package_manager)
    
    case "$pm" in
        apk)
            print_status "Running: apk update"
            apk update 2>&1 | tail -5 || true
            print_status "Running: apk add openjdk11..."
            if ! apk add --no-cache openjdk11-jre openjdk11-jdk 2>&1 | tail -10; then
                print_warning "Initial apk install failed, trying with sudo..."
                sudo apk update 2>&1 | tail -5 || true
                sudo apk add --no-cache openjdk11-jre openjdk11-jdk 2>&1 | tail -10 || {
                    print_error "Failed to install Java with apk"
                    return 1
                }
            fi
            ;;
        apt)
            apt-get update
            apt-get install -y openjdk-11-jre openjdk-11-jdk
            ;;
        yum)
            yum install -y java-11-openjdk java-11-openjdk-devel
            ;;
        brew)
            brew install openjdk@11
            ;;
        *)
            print_error "Unknown package manager"
            return 1
            ;;
    esac
    
    # Verify installation and update JAVA_HOME
    if command -v java &> /dev/null; then
        export JAVA_HOME="$(detect_java_home 2>/dev/null || echo '/usr/lib/jvm/java-11-openjdk')"
        export PATH="${JAVA_HOME}/bin:${PATH}"
        print_success "Java installed successfully at: ${JAVA_HOME}"
        return 0
    else
        print_error "Failed to install Java"
        return 1
    fi
}

# Download and install TLA+ tools
install_tlaplus() {
    if [[ -f "$TLATOOLS" ]]; then
        print_success "TLA+ toolbox already installed"
        return 0
    fi
    
    print_status "TLA+ toolbox not found. Installing..."
    
    # Create directory structure
    if ! mkdir -p /opt/tlaplus 2>/dev/null; then
        print_status "Trying with elevated privileges..."
        if ! sudo mkdir -p /opt/tlaplus 2>/dev/null; then
            print_error "Cannot create /opt/tlaplus directory (even with sudo)"
            # Try alternative location
            export TLATOOLS="${HOME}/.local/opt/tla2tools.jar"
            mkdir -p "${HOME}/.local/opt" || {
                print_error "Cannot create ${HOME}/.local/opt directory"
                return 1
            }
            print_info "Using alternative location: $TLATOOLS"
        fi
    fi
    
    # Download TLA+ (use specific version v1.8.0)
    local tlaplus_url="https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar"
    local temp_file="/tmp/tla2tools.jar"
    
    print_status "Downloading TLA+ from: $tlaplus_url"
    
    # Use curl if available, fall back to wget
    if command -v curl &> /dev/null; then
        curl -L --max-time 120 -o "$temp_file" "$tlaplus_url" 2>&1 | grep -E 'Could not resolve|Connection refused' && return 1
    elif command -v wget &> /dev/null; then
        wget --timeout=120 -O "$temp_file" "$tlaplus_url" 2>&1 | grep -E 'unable to resolve|Connection refused' && return 1
    else
        print_error "curl or wget is required to download TLA+. Installing..."
        install_packages curl
        curl -L --max-time 120 -o "$temp_file" "$tlaplus_url" 2>&1 | grep -E 'Could not resolve|Connection refused' && return 1
    fi
    
    if [[ ! -f "$temp_file" ]] || [[ ! -s "$temp_file" ]]; then
        print_error "Failed to download TLA+ toolbox (file missing or empty)"
        rm -f "$temp_file"
        return 1
    fi
    
    # Move to final location
    print_status "Installing TLA+ toolbox..."
    local dest_dir=$(dirname "$TLATOOLS")
    
    if [[ "$dest_dir" == "/opt/tlaplus" ]] && [[ ! -w "$dest_dir" ]]; then
        # Try sudo if we don't have write permissions to /opt/tlaplus
        if command -v sudo &> /dev/null; then
            sudo install -D -m 644 "$temp_file" "$TLATOOLS" || {
                print_error "Failed to install TLA+ toolbox to $TLATOOLS (even with sudo)"
                # Fall back to home directory
                export TLATOOLS="${HOME}/.local/opt/tla2tools.jar"
                mkdir -p "${HOME}/.local/opt" || return 1
                mv "$temp_file" "$TLATOOLS" || return 1
                print_info "Installed to alternative location: $TLATOOLS"
            }
        else
            mv "$temp_file" "$TLATOOLS" 2>/dev/null || {
                # Fall back to home directory
                export TLATOOLS="${HOME}/.local/opt/tla2tools.jar"
                mkdir -p "${HOME}/.local/opt" || return 1
                mv "$temp_file" "$TLATOOLS" || return 1
                print_info "Installed to alternative location: $TLATOOLS"
            }
        fi
    else
        mv "$temp_file" "$TLATOOLS" || {
            print_error "Failed to move TLA+ toolbox to $TLATOOLS"
            return 1
        }
    fi
    
    if [[ -f "$TLATOOLS" ]]; then
        print_success "TLA+ toolbox installed at $TLATOOLS"
        return 0
    else
        print_error "TLA+ toolbox installation failed"
        return 1
    fi
}

# Install system dependencies
install_system_deps() {
    print_status "Checking system dependencies..."
    
    local required_packages=()
    local pm=$(detect_package_manager)
    
    # Check for common tools
    if ! command -v git &> /dev/null; then
        print_warning "git not found"
        required_packages+=("git")
    else
        print_success "git found"
    fi
    
    if ! command -v curl &> /dev/null && ! command -v wget &> /dev/null; then
        print_warning "curl/wget not found"
        required_packages+=("curl")
    else
        print_success "curl/wget found"
    fi
    
    if ! command -v make &> /dev/null; then
        print_warning "make not found"
        case "$pm" in
            apk) required_packages+=("make") ;;
            apt) required_packages+=("make") ;;
            yum) required_packages+=("make") ;;
            brew) required_packages+=("make") ;;
        esac
    else
        print_success "make found"
    fi
    
    # Check for Rust/Cargo if applicable
    if [[ -f "/workspaces/Exo-OS/Cargo.toml" ]]; then
        if ! command -v cargo &> /dev/null; then
            print_warning "cargo not found (needed for Exo-OS)"
            print_info "Installing Rust toolchain..."
            if command -v curl &> /dev/null; then
                curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y 2>/dev/null
                source $HOME/.cargo/env
            else
                print_error "curl is required to install Rust"
                return 1
            fi
        else
            print_success "cargo found"
        fi
    fi
    
    # Install remaining packages if needed
    if [[ ${#required_packages[@]} -gt 0 ]]; then
        print_status "Installing missing system packages: ${required_packages[*]}"
        install_packages "${required_packages[@]}"
    else
        print_success "All system dependencies are present"
    fi
}

# Check Java installation
check_java() {
    if command -v java &> /dev/null; then
        local version=$(java -version 2>&1 | head -1)
        print_success "Java installed: $version"
        return 0
    else
        print_error "Java not found in PATH"
        return 1
    fi
}

# Check TLA+ installation
check_tlaplus() {
    if [[ -f "$TLATOOLS" ]]; then
        print_success "TLA+ toolbox found: $TLATOOLS"
        return 0
    else
        print_error "TLA+ toolbox not found at $TLATOOLS"
        return 1
    fi
}

# Verify all dependencies
verify_all_dependencies() {
    print_status "Verifying all dependencies..."
    local all_ok=true
    
    # Check Java
    if ! check_java; then
        all_ok=false
    fi
    
    # Check TLA+
    if ! check_tlaplus; then
        all_ok=false
    fi
    
    # Check other tools
    for tool in git curl make; do
        if command -v $tool &> /dev/null; then
            print_success "$tool found"
        else
            print_warning "$tool not found"
            all_ok=false
        fi
    done
    
    if [[ "$all_ok" == true ]]; then
        print_success "All dependencies verified"
        return 0
    else
        print_warning "Some dependencies are missing"
        return 1
    fi
}

# Run TLC model checker with given spec and config
run_tlc() {
    local spec=$1
    local config=$2
    local options=${3:-""}
    
    if [[ -z "$spec" ]]; then
        print_error "Usage: run_tlc <spec> [config] [options]"
        return 1
    fi
    
    cd /workspaces/Exo-OS/docs/Exo-OS-TLA+ || return 1
    
    print_status "Running TLC for $spec..."
    if [[ -n "$config" ]] && [[ -f "$config" ]]; then
        java -cp "$TLATOOLS" tlc2.TLC -config "$config" $options "$spec"
    else
        java -cp "$TLATOOLS" tlc2.TLC $options "$spec"
    fi
}

# Full setup/installation
setup_tla_environment() {
    echo ""
    echo -e "${MAGENTA}╔════════════════════════════════════════╗${NC}"
    echo -e "${MAGENTA}║  TLA+ Environment Full Installation   ║${NC}"
    echo -e "${MAGENTA}╚════════════════════════════════════════╝${NC}"
    echo ""
    
    # Step 1: System dependencies
    print_status "Step 1: Installing system dependencies..."
    if ! install_system_deps; then
        print_warning "Some system dependencies could not be installed, continuing..."
    fi
    echo ""
    
    # Step 2: Java
    print_status "Step 2: Setting up Java..."
    if ! install_java; then
        print_error "Failed to install Java (required for TLA+)"
        return 1
    fi
    echo ""
    
    # Step 3: TLA+
    print_status "Step 3: Installing TLA+ toolbox..."
    if ! install_tlaplus; then
        print_error "Failed to install TLA+ toolbox"
        return 1
    fi
    echo ""
    
    # Step 4: Verification
    print_status "Step 4: Verifying installation..."
    if verify_all_dependencies; then
        echo ""
        echo -e "${GREEN}╔════════════════════════════════════════╗${NC}"
        echo -e "${GREEN}║  ✓ Installation Complete!             ║${NC}"
        echo -e "${GREEN}╚════════════════════════════════════════╝${NC}"
        echo ""
        print_success "You can now use TLA+ for Exo-OS models"
        print_info "Try: run_tlc ExoOS_Full ExoOS_Composition.cfg"
        return 0
    else
        print_error "Verification failed. Some dependencies may not be properly installed."
        return 1
    fi
}

# Print environment info if "testing" argument is passed
if [[ "$1" == "setup" ]] || [[ "$1" == "install" ]]; then
    # Full installation mode
    if setup_tla_environment; then
        export TLA_ENV_INSTALLED="1"
    else
        print_error "Setup failed"
        exit 1
    fi
elif [[ "$1" == "testing" ]] || [[ "$1" == "check" ]]; then
    echo ""
    echo -e "${BLUE}=== TLA+ Environment Check ===${NC}"
    echo ""
    
    verify_all_dependencies
    
    echo ""
    echo -e "${BLUE}Environment Variables:${NC}"
    echo "JAVA_HOME: $JAVA_HOME"
    echo "TLATOOLS: $TLATOOLS"
    echo "PATH includes: /opt/tlaplus"
    
    echo ""
    echo -e "${BLUE}Available TLA+ Specifications:${NC}"
    if [[ -d /workspaces/Exo-OS/docs/Exo-OS-TLA+ ]]; then
        cd /workspaces/Exo-OS/docs/Exo-OS-TLA+ 2>/dev/null
        ls -1 *.tla 2>/dev/null | sed 's/^/  - /' || echo "  (none found)"
        
        echo ""
        echo -e "${BLUE}Configuration Files:${NC}"
        ls -1 *.cfg 2>/dev/null | sed 's/^/  - /' || echo "  (none found)"
    else
        echo "  TLA+ directory not found"
    fi
    
    echo ""
    echo -e "${BLUE}Available Commands:${NC}"
    echo "  run_tlc ExoOS_Full ExoOS_Composition.cfg      # Run TLC with spec and config"
    echo "  run_tlc ExoOS_Full                            # Run TLC with spec only"
    echo "  java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -help  # Show TLC options"
    echo ""
    
    echo -e "${BLUE}Setup Options:${NC}"
    echo "  source tla_env.sh setup    # Full installation (if dependencies missing)"
    echo "  source tla_env.sh check    # Verify environment"
    echo "  source tla_env.sh testing  # Same as check"
    echo ""
fi

print_success "TLA+ environment ready!"
