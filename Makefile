# Makefile pour Exo-OS
# Usage: make <target>

.PHONY: all build clean test run qemu help

# Configuration
KERNEL_DIR = kernel
TARGET = x86_64-unknown-none.json
CARGO = cargo +nightly
BUILD_FLAGS = --target ../$(TARGET) -Z build-std=core,alloc,compiler_builtins

# Couleurs pour l'output
BLUE = \033[0;34m
GREEN = \033[0;32m
YELLOW = \033[0;33m
RED = \033[0;31m
NC = \033[0m # No Color

all: build

## Build du kernel
build:
	@echo "$(BLUE)🔨 Compilation du kernel Exo-OS...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) build $(BUILD_FLAGS)
	@echo "$(GREEN)✅ Compilation réussie!$(NC)"

## Build en mode release
release:
	@echo "$(BLUE)🚀 Compilation en mode release...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) build --release $(BUILD_FLAGS)
	@echo "$(GREEN)✅ Build release terminé!$(NC)"

## Nettoyer les fichiers de build
clean:
	@echo "$(YELLOW)🧹 Nettoyage...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) clean
	@rm -rf iso/ exo-os.iso
	@echo "$(GREEN)✅ Nettoyage terminé!$(NC)"

## Vérifier le code (clippy + format)
check:
	@echo "$(BLUE)🔍 Vérification du code...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) fmt --check
	@cd $(KERNEL_DIR) && $(CARGO) clippy $(BUILD_FLAGS) -- -D warnings
	@echo "$(GREEN)✅ Code vérifié!$(NC)"

## Formatter le code
fmt:
	@echo "$(BLUE)✨ Formatage du code...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) fmt
	@echo "$(GREEN)✅ Code formaté!$(NC)"

## Lancer les tests
test:
	@echo "$(BLUE)🧪 Exécution des tests...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) test $(BUILD_FLAGS)
	@echo "$(GREEN)✅ Tests terminés!$(NC)"

## Créer une image bootable (nécessite bootimage)
bootimage: build
	@echo "$(BLUE)📦 Création de l'image bootable...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) bootimage
	@echo "$(GREEN)✅ Image créée!$(NC)"

## Lancer avec QEMU (nécessite bootimage)
qemu: bootimage
	@echo "$(BLUE)🖥️  Lancement de QEMU...$(NC)"
	@echo "$(YELLOW)Pour quitter: Ctrl+A puis X$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) bootimage --run

## Test rapide avec PowerShell
test-ps:
	@pwsh -File test-qemu.ps1

## Afficher les informations sur le build
info:
	@echo "$(BLUE)ℹ️  Informations Exo-OS$(NC)"
	@echo "  Kernel dir: $(KERNEL_DIR)"
	@echo "  Target: $(TARGET)"
	@echo "  Cargo: $(CARGO)"
	@echo ""
	@echo "$(BLUE)📊 Taille du kernel:$(NC)"
	@ls -lh $(KERNEL_DIR)/target/x86_64-unknown-none/debug/libexo_kernel.a 2>/dev/null || echo "  (pas encore compilé)"

## Afficher les warnings de compilation
warnings:
	@echo "$(BLUE)⚠️  Compilation avec warnings détaillés...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) build $(BUILD_FLAGS) 2>&1 | grep "warning:"

## Afficher ce message d'aide
help:
	@echo "$(BLUE)╔═══════════════════════════════════════════╗$(NC)"
	@echo "$(BLUE)║          Exo-OS Makefile Help            ║$(NC)"
	@echo "$(BLUE)╚═══════════════════════════════════════════╝$(NC)"
	@echo ""
	@echo "$(GREEN)Commandes principales:$(NC)"
	@echo "  make build      - Compiler le kernel"
	@echo "  make release    - Compiler en mode release"
	@echo "  make test       - Lancer les tests"
	@echo "  make clean      - Nettoyer les fichiers de build"
	@echo ""
	@echo "$(GREEN)Testing:$(NC)"
	@echo "  make bootimage  - Créer une image bootable"
	@echo "  make qemu       - Lancer avec QEMU"
	@echo "  make test-ps    - Test avec PowerShell script"
	@echo ""
	@echo "$(GREEN)Qualité du code:$(NC)"
	@echo "  make check      - Vérifier (clippy + format)"
	@echo "  make fmt        - Formatter le code"
	@echo "  make warnings   - Voir les warnings"
	@echo ""
	@echo "$(GREEN)Utilitaires:$(NC)"
	@echo "  make info       - Infos sur le build"
	@echo "  make help       - Afficher cette aide"
	@echo ""
	@echo "$(YELLOW)Note:$(NC) Certaines commandes nécessitent:"
	@echo "  - bootimage: $(YELLOW)cargo install bootimage$(NC)"
	@echo "  - QEMU: $(YELLOW)https://qemu.org$(NC)"
