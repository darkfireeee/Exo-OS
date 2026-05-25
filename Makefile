# ─── Makefile Exo-OS ──────────────────────────────────────────────────────────
# Usage : make <cible>
#
# Flux de build complet :
#   make build   → compile le kernel ELF (cargo, x86_64-unknown-none)
#   make iso     → construit exo-os.iso (GRUB 2 Multiboot2, grub-mkrescue)
#   make qemu    → lance QEMU depuis l'ISO (x86_64, 256M RAM, sortie série stdio)
#   make run     → alias de qemu

.PHONY: all build build-boot-payloads release iso iso-phoenix-resurrection iso-release-phoenix-resurrection qemu qemu-e1000 qemu-virtio-net qemu-nographic-virtio-net qemu-headless-safe-virtio-net run clean check fmt test test-exofs test-userspace test-drivers test-loader qemu-shell-smoke info help qemu-headless-safe qemu-phoenix-resurrection qemu-release-phoenix-resurrection

# ── Outils ───────────────────────────────────────────────────────────────────
CARGO          = cargo
KERNEL_DIR     = kernel
ISO_WORKDIR    = iso_build
ISO_OUTPUT     = exo-os.iso
BAREMETAL_TARGET ?= x86_64-unknown-none
USERSPACE_TARGET_JSON ?= $(abspath x86_64-exo-userspace.json)
USERSPACE_TARGET_DIR  ?= x86_64-exo-userspace
CARGO_TEST_FLAGS = -Z panic-abort-tests
CARGO_BAREMETAL_FLAGS = -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem
CARGO_USERSPACE_FLAGS = -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem -Z json-target-spec
KERNEL_PAYLOAD_CFG = --config 'target.x86_64-unknown-none.rustflags=["--cfg","exo_boot_payloads"]'
HOST_TEST_TARGET ?= x86_64-unknown-linux-gnu
HOST_TEST_OVERRIDES = --target $(HOST_TEST_TARGET)

# Kernel buildé par cargo (dans le workspace target/)
KERNEL_BIN_DBG  = target/x86_64-unknown-none/debug/exo-os-kernel
KERNEL_BIN_REL  = target/x86_64-unknown-none/release/exo-os-kernel
KERNEL_A_DBG    = target/exophoenix/kernel-a-debug.elf
KERNEL_A_REL    = target/exophoenix/kernel-a-release.elf
BOOT_PAYLOAD_RAW_DIR = target/$(USERSPACE_TARGET_DIR)/debug
BOOT_PAYLOAD_DIR = target/boot-payloads-stripped
STRIP_TOOL ?= $(shell command -v llvm-strip 2>/dev/null || command -v strip 2>/dev/null || echo :)
BOOT_SERVER_PACKAGES = \
	-p exo-init-server \
	-p exo-ipc-router \
	-p exo-memory-server \
	-p exo-vfs-server \
	-p exo-crypto-server \
	-p exo-device-server \
	-p exo-virtio-drivers \
	-p exo-e1000-driver \
	-p exo-virtio-net-driver \
	-p exo-loopback-driver \
	-p exo-network-server \
	-p exo-scheduler-server \
	-p exo-input-server \
	-p exo-tty-server \
	-p exo-exosh \
	-p exo-shield \
	-p exo-loader
BOOT_PAYLOAD_FEATURES = --features exo-loader/dynamic_linking
BOOT_PAYLOAD_BINS = \
	exo-init-server \
	exo-ipc-router \
	exo-memory-server \
	exo-vfs-server \
	exo-crypto-server \
	exo-device-server \
	exo-virtio-drivers \
	exo-e1000-driver \
	exo-virtio-net-driver \
	exo-loopback-driver \
	exo-network-server \
	exo-scheduler-server \
	exo-input-server \
	exo-tty-server \
	exosh \
	exo-shield \
	exo-loader

# ── QEMU ─────────────────────────────────────────────────────────────────────
# Paramètres QEMU communs (machine Q35 moderne, 256 MiB, VGA standard)
QEMU = qemu-system-x86_64
QEMU_FLAGS  = -machine q35
QEMU_FLAGS += -m 256M
QEMU_FLAGS += -boot d
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
QEMU_EXOFS_DISK      ?= target/qemu/exofs-root.img
QEMU_EXOFS_DISK_SIZE ?= 512M
QEMU_EXOFS_DRIVE_FLAGS = -drive if=none,file=$(QEMU_EXOFS_DISK),format=raw,id=exofs0,cache=writeback -device virtio-blk-pci,drive=exofs0
QEMU_E1000_NET_FLAGS    = -netdev user,id=exonet0 -device e1000,netdev=exonet0,mac=02:45:58:4f:00:01
QEMU_VIRTIO_NET_FLAGS   = -netdev user,id=exonet0 -device virtio-net-pci-non-transitional,netdev=exonet0,mac=02:45:58:4f:00:01
QEMU_NET_FLAGS          ?= $(QEMU_VIRTIO_NET_FLAGS)
ifeq ($(strip $(QEMU_NET_FLAGS)),)
override QEMU_NET_FLAGS := $(QEMU_VIRTIO_NET_FLAGS)
endif
QEMU_HEADLESS_SAFE_FLAGS  = -machine q35
QEMU_HEADLESS_SAFE_FLAGS += -m 256M
QEMU_HEADLESS_SAFE_FLAGS += -boot d
QEMU_HEADLESS_SAFE_FLAGS += -vga std
QEMU_HEADLESS_SAFE_FLAGS += -serial file:$(QEMU_SAFE_SERIAL_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -no-reboot
QEMU_HEADLESS_SAFE_FLAGS += -no-shutdown
QEMU_HEADLESS_SAFE_FLAGS += -d int,cpu_reset -D $(QEMU_SAFE_INT_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -debugcon file:$(QEMU_SAFE_E9_LOG)
QEMU_HEADLESS_SAFE_FLAGS += -device isa-debug-exit,iobase=0xf4,iosize=0x04

$(QEMU_EXOFS_DISK):
	@mkdir -p $(dir $(QEMU_EXOFS_DISK))
	@if [ ! -f "$(QEMU_EXOFS_DISK)" ]; then \
	    echo "$(BLUE)Creation disque ExoFS QEMU : $(QEMU_EXOFS_DISK) ($(QEMU_EXOFS_DISK_SIZE))$(NC)"; \
	    truncate -s $(QEMU_EXOFS_DISK_SIZE) "$(QEMU_EXOFS_DISK)"; \
	fi

# Couleurs
BLUE   = \033[0;34m
GREEN  = \033[0;32m
YELLOW = \033[0;33m
RED    = \033[0;31m
CYAN   = \033[0;36m
NC     = \033[0m

# ── Cibles ───────────────────────────────────────────────────────────────────

all: iso

## 1a. Build des payloads Ring1 embarques dans le rootfs ExoFS de boot
build-boot-payloads:
	@echo "$(BLUE)[payloads] Compilation serveurs Ring1 pour l'injection /sbin...$(NC)"
	@$(CARGO) build $(BOOT_SERVER_PACKAGES) $(BOOT_PAYLOAD_FEATURES) --target $(USERSPACE_TARGET_JSON) $(CARGO_USERSPACE_FLAGS)
	@rm -rf "$(BOOT_PAYLOAD_DIR)"
	@mkdir -p "$(BOOT_PAYLOAD_DIR)"
	@for bin in $(BOOT_PAYLOAD_BINS); do \
		cp "$(BOOT_PAYLOAD_RAW_DIR)/$$bin" "$(BOOT_PAYLOAD_DIR)/$$bin"; \
		if [ "$(STRIP_TOOL)" != ":" ]; then "$(STRIP_TOOL)" --strip-all "$(BOOT_PAYLOAD_DIR)/$$bin" 2>/dev/null || true; fi; \
	done
	@echo "$(GREEN)[OK] Payloads /sbin prets : $(BOOT_PAYLOAD_DIR)$(NC)"

## 1. Build debug du kernel (rapide, symboles complets)
build: build-boot-payloads
	@echo "$(BLUE)[1/2] Compilation Kernel A propre ExoPhoenix (release, image de résurrection)...$(NC)"
	@mkdir -p target/exophoenix
	@cd $(KERNEL_DIR) && EXOPHOENIX_BUILD_ROLE=A $(CARGO) build --release --target $(BAREMETAL_TARGET) $(CARGO_BAREMETAL_FLAGS)
	@cp $(KERNEL_BIN_REL) $(KERNEL_A_DBG)
	@echo "$(BLUE)[2/2] Compilation Kernel B avec image Kernel A injectée (debug)...$(NC)"
	@cd $(KERNEL_DIR) && KERNEL_A_IMAGE_PATH="$(abspath $(KERNEL_A_DBG))" EXO_BOOT_PAYLOAD_DIR="$(abspath $(BOOT_PAYLOAD_DIR))" EXOPHOENIX_RESCUE_TEST="$(EXOPHOENIX_RESCUE_TEST)" $(CARGO) build $(KERNEL_PAYLOAD_CFG) --target $(BAREMETAL_TARGET) $(CARGO_BAREMETAL_FLAGS)
	@echo "$(GREEN)[OK] Kernel compilé : $(KERNEL_BIN_DBG)$(NC)"

## 2. Build release du kernel (optimisé, LTO, plus petit)
release: build-boot-payloads
	@echo "$(BLUE)[1/2] Compilation Kernel A propre ExoPhoenix (release)...$(NC)"
	@mkdir -p target/exophoenix
	@cd $(KERNEL_DIR) && EXOPHOENIX_BUILD_ROLE=A $(CARGO) build --release --target $(BAREMETAL_TARGET) $(CARGO_BAREMETAL_FLAGS)
	@cp $(KERNEL_BIN_REL) $(KERNEL_A_REL)
	@echo "$(BLUE)[2/2] Compilation Kernel B avec image Kernel A injectée (release)...$(NC)"
	@cd $(KERNEL_DIR) && KERNEL_A_IMAGE_PATH="$(abspath $(KERNEL_A_REL))" EXO_BOOT_PAYLOAD_DIR="$(abspath $(BOOT_PAYLOAD_DIR))" EXOPHOENIX_RESCUE_TEST="$(EXOPHOENIX_RESCUE_TEST)" $(CARGO) build $(KERNEL_PAYLOAD_CFG) --release --target $(BAREMETAL_TARGET) $(CARGO_BAREMETAL_FLAGS)
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

## 3c. ISO debug avec autodestruction/résurrection ExoPhoenix activée
iso-phoenix-resurrection:
	@$(MAKE) --no-print-directory iso EXOPHOENIX_RESCUE_TEST=1 ISO_OUTPUT=exo-os-phoenix.iso

## 3d. ISO release avec autodestruction/résurrection ExoPhoenix activée
iso-release-phoenix-resurrection:
	@$(MAKE) --no-print-directory iso-release EXOPHOENIX_RESCUE_TEST=1 ISO_OUTPUT=exo-os-phoenix-release.iso

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
qemu: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU VirtIO-net — Ctrl+C pour quitter$(NC)"
	@echo "$(YELLOW)Log interruptions : /tmp/qemu-exoos.log$(NC)"
	@echo "$(BLUE)Net flags : $(QEMU_NET_FLAGS)$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_NET_FLAGS) -cdrom $(ISO_OUTPUT)

## 4a. Lancer QEMU avec le transport reseau Intel e1000
qemu-e1000: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU e1000 — Ctrl+C pour quitter$(NC)"
	@echo "$(YELLOW)Log interruptions : /tmp/qemu-exoos.log$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_E1000_NET_FLAGS) -cdrom $(ISO_OUTPUT)

## 4a. Lancer QEMU avec le transport reseau VirtIO PCI
qemu-virtio-net: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU VirtIO-net — Ctrl+C pour quitter$(NC)"
	@echo "$(YELLOW)Log interruptions : /tmp/qemu-exoos.log$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_VIRTIO_NET_FLAGS) -cdrom $(ISO_OUTPUT)

## 4b. Lancer en mode release
qemu-release: iso-release $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU (release)$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_NET_FLAGS) -cdrom $(ISO_OUTPUT)

## 4c. Lancer QEMU sans fenêtre graphique (serveur headless)
qemu-nographic: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU headless (sortie texte)$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_NET_FLAGS) -cdrom $(ISO_OUTPUT) -nographic -display none

qemu-nographic-virtio-net: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU VirtIO-net headless (sortie texte)$(NC)"
	$(QEMU) $(QEMU_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_VIRTIO_NET_FLAGS) -cdrom $(ISO_OUTPUT) -nographic -display none

## 4d. Lancer QEMU headless "zéro surprise" (logs fichiers dédiés)
qemu-headless-safe: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU headless sûr (logs fichiers, sans stdio partagé)$(NC)"
	@echo "$(YELLOW)Serial  : $(QEMU_SAFE_SERIAL_LOG)$(NC)"
	@echo "$(YELLOW)INT log : $(QEMU_SAFE_INT_LOG)$(NC)"
	@echo "$(YELLOW)E9 log  : $(QEMU_SAFE_E9_LOG)$(NC)"
	$(QEMU) $(QEMU_HEADLESS_SAFE_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_NET_FLAGS) -cdrom $(ISO_OUTPUT) -display none

qemu-headless-safe-virtio-net: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Lancement QEMU VirtIO-net headless sûr (logs fichiers, sans stdio partagé)$(NC)"
	@echo "$(YELLOW)Serial  : $(QEMU_SAFE_SERIAL_LOG)$(NC)"
	@echo "$(YELLOW)INT log : $(QEMU_SAFE_INT_LOG)$(NC)"
	@echo "$(YELLOW)E9 log  : $(QEMU_SAFE_E9_LOG)$(NC)"
	$(QEMU) $(QEMU_HEADLESS_SAFE_FLAGS) $(QEMU_EXOFS_DRIVE_FLAGS) $(QEMU_VIRTIO_NET_FLAGS) -cdrom $(ISO_OUTPUT) -display none

## 4e. Lancer le test de résurrection ExoPhoenix en QEMU headless
qemu-phoenix-resurrection: iso-phoenix-resurrection
	@echo "$(CYAN)Lancement QEMU test ExoPhoenix résurrection$(NC)"
	@echo "$(YELLOW)Serial  : $(QEMU_SAFE_SERIAL_LOG)$(NC)"
	@echo "$(YELLOW)INT log : $(QEMU_SAFE_INT_LOG)$(NC)"
	@echo "$(YELLOW)E9 log  : $(QEMU_SAFE_E9_LOG)$(NC)"
	$(QEMU) $(QEMU_HEADLESS_SAFE_FLAGS) -cdrom exo-os-phoenix.iso -display none

## 4f. Lancer le test de résurrection ExoPhoenix en release headless
qemu-release-phoenix-resurrection: iso-release-phoenix-resurrection
	@echo "$(CYAN)Lancement QEMU test ExoPhoenix résurrection (release)$(NC)"
	@echo "$(YELLOW)Serial  : $(QEMU_SAFE_SERIAL_LOG)$(NC)"
	@echo "$(YELLOW)INT log : $(QEMU_SAFE_INT_LOG)$(NC)"
	@echo "$(YELLOW)E9 log  : $(QEMU_SAFE_E9_LOG)$(NC)"
	$(QEMU) $(QEMU_HEADLESS_SAFE_FLAGS) -cdrom exo-os-phoenix-release.iso -display none

run: qemu

# ── Tests & qualité ──────────────────────────────────────────────────────────
## Vérifier (clippy)
check:
	@echo "$(BLUE)Vérification clippy...$(NC)"
	@cd $(KERNEL_DIR) && EXOPHOENIX_BUILD_ROLE=A $(CARGO) clippy --target $(BAREMETAL_TARGET) $(CARGO_BAREMETAL_FLAGS)
	@echo "$(GREEN)[OK]$(NC)"

## Formatter le code
fmt:
	@echo "$(BLUE)Formatage...$(NC)"
	@cd $(KERNEL_DIR) && $(CARGO) fmt
	@echo "$(GREEN)[OK]$(NC)"

## Tests unitaires (host, pas bare-metal)
test:
	@echo "$(BLUE)Tests unitaires...$(NC)"
	@cd $(KERNEL_DIR) && EXOPHOENIX_BUILD_ROLE=A $(CARGO) test --lib $(CARGO_TEST_FLAGS) $(HOST_TEST_OVERRIDES)

## Tests ExoFS ciblés (corrige duplicate-core via panic-abort-tests)
test-exofs:
	@echo "$(BLUE)Tests ExoFS (filtre fs::exofs::)...$(NC)"
	@cd $(KERNEL_DIR) && EXOPHOENIX_BUILD_ROLE=A $(CARGO) test --lib $(CARGO_TEST_FLAGS) $(HOST_TEST_OVERRIDES) fs::exofs::

test-drivers:
	@echo "$(BLUE)Tests drivers shell/QEMU...$(NC)"
	@$(CARGO) test --manifest-path drivers/input/ps2/Cargo.toml
	@$(CARGO) test --manifest-path drivers/tty/Cargo.toml
	@$(CARGO) test --manifest-path drivers/display/vga/Cargo.toml
	@$(CARGO) test --manifest-path drivers/display/framebuffer/Cargo.toml
	@$(CARGO) test --manifest-path drivers/storage/virtio_blk/Cargo.toml

test-loader:
	@echo "$(BLUE)Tests loader ELF...$(NC)"
	@$(CARGO) test --manifest-path loader/Cargo.toml

test-userspace:
	@echo "$(BLUE)Tests userspace shell/coreutils...$(NC)"
	@cd userspace && $(CARGO) test --workspace

qemu-shell-smoke: iso $(QEMU_EXOFS_DISK)
	@echo "$(CYAN)Smoke QEMU shell avec disque virtio persistant$(NC)"
	@bash scripts/qemu/shell_smoke_qmp.sh "$(QEMU_EXOFS_DISK)"

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
	@echo "$(GREEN)  make qemu$(NC)          Lancer Exo-OS dans QEMU VirtIO-net (debug)"
	@echo "$(GREEN)  make qemu-e1000$(NC)    Lancer Exo-OS dans QEMU e1000 (debug)"
	@echo "$(GREEN)  make qemu-virtio-net$(NC) Lancer Exo-OS dans QEMU VirtIO-net (debug)"
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
	@echo "  make qemu       - Lancer avec QEMU VirtIO-net"
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
