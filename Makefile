# ─── Makefile Exo-OS ──────────────────────────────────────────────────────────
# Usage : make <cible>
#
# Flux de build complet :
#   make build   → compile le kernel ELF (cargo, x86_64-unknown-none)
#   make iso     → construit exo-os.iso (GRUB 2 Multiboot2, grub-mkrescue)
#   make qemu    → lance QEMU depuis l'ISO (x86_64, 256M RAM, sortie série stdio)
#   make run     → alias de qemu

.PHONY: all build release iso qemu run clean check fmt test test-exofs info help qemu-headless-safe

# ── Outils ───────────────────────────────────────────────────────────────────
CARGO          = cargo
KERNEL_DIR     = kernel
ISO_WORKDIR    = iso_build
ISO_OUTPUT     = exo-os.iso
CARGO_TEST_FLAGS = -Z panic-abort-tests
CARGO_BAREMETAL_FLAGS = -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem
HOST_TEST_TARGET ?= x86_64-unknown-linux-gnu
HOST_TEST_OVERRIDES = --target $(HOST_TEST_TARGET)

# Kernel buildé par cargo (dans le workspace target/)
KERNEL_BIN_DBG  = target/x86_64-unknown-none/debug/exo-os-kernel
KERNEL_BIN_REL  = target/x86_64-unknown-none/release/exo-os-kernel

# ── QEMU ─────────────────────────────────────────────────────────────────────
# Paramètres QEMU communs (machine Q35 moderne, 256 MiB, VGA standard)
QEMU = qemu-system-x86_64
QEMU_FLAGS  = -machine q35
QEMU_FLAGS += -m 256M
QEMU_FLAGS += -vga std
QEMU_FLAGS += -serial stdio
QEMU_FLAGS += -no-reboot
QEMU_FLAGS += -no-shutdown
QEMU_FLAGS += -d int,cpu_reset -D /tmp/qemu-exoos.log
# Sortie debug sur port 0xE9 → fichier /tmp/e9k.txt (debugcon QEMU)
QEMU_FLAGS += -debugcon file:/tmp/e9k.txt
# isa-debug-exit : permet à l'OS de signaler le code de sortie à QEMU
QEMU_FLAGS += -device isa-debug-exit,iobase=0xf4,iosize=0x04

# Variante headless "zéro surprise" : pas de stdio partagé.
# Utiliser des fichiers de logs dédiés pour éviter le conflit
# "cannot use stdio by multiple character devices".
QEMU_SAFE_SERIAL_LOG ?= /tmp/exoos-serial.log
QEMU_SAFE_INT_LOG    ?= /tmp/exoos-qemu-int.log
QEMU_SAFE_E9_LOG     ?= /tmp/exoos-e9.log
QEMU_HEADLESS_SAFE_FLAGS  = -machine q35
QEMU_HEADLESS_SAFE_FLAGS += -m 256M
QEMU_HEADLESS_SAFE_FLAGS += -vga std
QEMU_HEADLESS_SAFE_FLAGS += -serial file:$(QEMU_SAFE_SERIAL_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -no-reboot
QEMU_HEADLESS_SAFE_FLAGS += -no-shutdown
QEMU_HEADLESS_SAFE_FLAGS += -d int,cpu_reset -D $(QEMU_SAFE_INT_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -debugcon file:$(QEMU_SAFE_E9_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -device isa-debug-exit,iobase=0xf4,iosize=0x04

# Couleurs
BLUE   = \033[0;34m
GREEN  = \033[0;32m
YELLOW = \033[0;33m
RED    = \033[0;31m
CYAN   = \033[0;36m
NC     = \033[0m

# ── Cibles ───────────────────────────────────────────────────────────────────

all: iso

## 1. Build debug du kernel (rapide, symboles complets)
build:
	@echo "$(BLUE)[1/1] Compilation kernel Exo-OS (debug)...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) build $(CARGO_BAREMETAL_FLAGS)
	@echo "$(GREEN)[OK] Kernel compilé : $(KERNEL_BIN_DBG)$(NC)"

## 2. Build release du kernel (optimisé, LTO, plus petit)
release:
	@echo "$(BLUE)[1/1] Compilation kernel Exo-OS (release)...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) build --release $(CARGO_BAREMETAL_FLAGS)
	@echo "$(GREEN)[OK] Kernel compilé : $(KERNEL_BIN_REL)$(NC)"

# ── Cible ISO (debug) ─────────────────────────────────────────────────────────
## 3. Construire l'image ISO bootable avec GRUB 2 (Multiboot2)
iso: build
	@echo "$(BLUE)[2/2] Construction ISO GRUB 2 (Multiboot2)...$(NC)"
	@$(MAKE) --no-print-directory _make_iso KERNEL_BIN=$(KERNEL_BIN_DBG)
	@echo "$(GREEN)[OK] ISO créée : $(ISO_OUTPUT)$(NC)"
	@ls -lh $(ISO_OUTPUT)

## 3b. Construire l'image ISO en mode release
iso-release: release
	@echo "$(BLUE)[2/2] Construction ISO release...$(NC)"
	@$(MAKE) --no-print-directory _make_iso KERNEL_BIN=$(KERNEL_BIN_REL)
	@echo "$(GREEN)[OK] ISO release créée : $(ISO_OUTPUT)$(NC)"
	@ls -lh $(ISO_OUTPUT)

# Sous-cible interne : assemble l'ISO depuis le KERNEL_BIN fourni en paramètre.
_make_iso:
	@rm -rf $(ISO_WORKDIR)
	@mkdir -p $(ISO_WORKDIR)/boot/grub
	@cp $(KERNEL_BIN) $(ISO_WORKDIR)/boot/exo-os-kernel
	@cp bootloader/grub.cfg $(ISO_WORKDIR)/boot/grub/grub.cfg
	@grub-mkrescue -o $(ISO_OUTPUT) $(ISO_WORKDIR) \
	    --compress=xz 2>&1 | grep -v "^$$" || true
	@rm -rf $(ISO_WORKDIR)

# ── Lancement QEMU ────────────────────────────────────────────────────────────
## 4. Lancer Exo-OS dans QEMU (depuis l'ISO debug)
qemu: iso
	@echo "$(CYAN)Lancement QEMU — Ctrl+C pour quitter$(NC)"
	@echo "$(YELLOW)Log interruptions : /tmp/qemu-exoos.log$(NC)"
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_OUTPUT)

## 4b. Lancer en mode release
qemu-release: iso-release
	@echo "$(CYAN)Lancement QEMU (release)$(NC)"
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_OUTPUT)

## 4c. Lancer QEMU sans fenêtre graphique (serveur headless)
qemu-nographic: iso
	@echo "$(CYAN)Lancement QEMU headless (sortie texte)$(NC)"
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_OUTPUT) -nographic -display none

## 4d. Lancer QEMU headless "zéro surprise" (logs fichiers dédiés)
qemu-headless-safe: iso
	@echo "$(CYAN)Lancement QEMU headless sûr (logs fichiers, sans stdio partagé)$(NC)"
	@echo "$(YELLOW)Serial  : $(QEMU_SAFE_SERIAL_LOG)$(NC)"
	@echo "$(YELLOW)INT log : $(QEMU_SAFE_INT_LOG)$(NC)"
	@echo "$(YELLOW)E9 log  : $(QEMU_SAFE_E9_LOG)$(NC)"
	$(QEMU) $(QEMU_HEADLESS_SAFE_FLAGS) -cdrom $(ISO_OUTPUT) -display none

run: qemu

# ── Tests & qualité ──────────────────────────────────────────────────────────
## Vérifier (clippy)
check:
	@echo "$(BLUE)Vérification clippy...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) clippy $(CARGO_BAREMETAL_FLAGS)
	@echo "$(GREEN)[OK]$(NC)"

## Formatter le code
fmt:
	@echo "$(BLUE)Formatage...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) fmt
	@echo "$(GREEN)[OK]$(NC)"

## Tests unitaires (host, pas bare-metal)
test:
	@echo "$(BLUE)Tests unitaires...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) test --lib $(CARGO_TEST_FLAGS) $(HOST_TEST_OVERRIDES)

## Tests ExoFS ciblés (corrige duplicate-core via panic-abort-tests)
test-exofs:
	@echo "$(BLUE)Tests ExoFS (filtre fs::exofs::)...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) test --lib $(CARGO_TEST_FLAGS) $(HOST_TEST_OVERRIDES) fs::exofs::

# ── Nettoyage ─────────────────────────────────────────────────────────────────
clean:
	@echo "$(YELLOW)Nettoyage...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) clean
	@rm -rf $(ISO_WORKDIR) $(ISO_OUTPUT) /tmp/qemu-exoos.log $(QEMU_SAFE_SERIAL_LOG) $(QEMU_SAFE_INT_LOG) $(QEMU_SAFE_E9_LOG)
	@echo "$(GREEN)[OK]$(NC)"

# ── Info ──────────────────────────────────────────────────────────────────────
info:
	@echo "$(CYAN)"
	@echo "  ___                 ___  ____  "
	@echo " |  _| _  _____     / _ \\/ ___| "
	@echo " | |_ \\ \\/ / _ \\___| | | \\___ \\"
	@echo " |  _| >  < (_) |___| |_| |___) |"
	@echo " |___//_/\\_\\___/     \\___/|____/ "
	@echo "$(NC)"
	@echo "$(BLUE)Exo-OS — Informations de build$(NC)"
	@echo "  Kernel dir : $(KERNEL_DIR)"
	@echo "  Target     : x86_64-unknown-none"
	@echo "  ISO        : $(ISO_OUTPUT)"
	@echo ""
	@echo "$(BLUE)Taille kernel debug :$(NC)"
	@ls -lh $(KERNEL_BIN_DBG) 2>/dev/null || echo "  (pas encore compilé)"
	@echo "$(BLUE)Taille kernel release :$(NC)"
	@ls -lh $(KERNEL_BIN_REL) 2>/dev/null || echo "  (pas encore compilé)"
	@echo ""
	@echo "$(BLUE)GRUB : $(shell grub-mkrescue --version 2>/dev/null || echo non installé)$(NC)"
	@echo "$(BLUE)QEMU : $(shell qemu-system-x86_64 --version 2>/dev/null | head -1 || echo non installé)$(NC)"

help:
	@echo "$(CYAN)Exo-OS — Cibles Makefile$(NC)"
	@echo ""
	@echo "$(GREEN)  make build$(NC)         Compiler le kernel (debug)"
	@echo "$(GREEN)  make release$(NC)       Compiler le kernel (release optimisé)"
	@echo "$(GREEN)  make iso$(NC)           Construire exo-os.iso (debug)"
	@echo "$(GREEN)  make iso-release$(NC)   Construire exo-os.iso (release)"
	@echo "$(GREEN)  make qemu$(NC)          Lancer Exo-OS dans QEMU (debug)"
	@echo "$(GREEN)  make qemu-release$(NC)  Lancer Exo-OS dans QEMU (release)"
	@echo "$(GREEN)  make qemu-nographic$(NC)Lancer sans interface graphique"
	@echo "$(GREEN)  make qemu-headless-safe$(NC) Lancer headless avec logs dédiés"
	@echo "$(GREEN)  make clean$(NC)         Nettoyer les artefacts"
	@echo "$(GREEN)  make check$(NC)         Vérification clippy"
	@echo "$(GREEN)  make test$(NC)          Tests unitaires kernel (panic-abort-tests)"
	@echo "$(GREEN)  make test-exofs$(NC)    Tests ExoFS filtrés"
	@echo "$(GREEN)  make info$(NC)          Informations sur le build"
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
