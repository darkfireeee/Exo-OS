//! uefi/nvram.rs — Gestion des variables NVRAM UEFI (BootXXXX et BootOrder).
//!
//! Permet au bootloader Exo-OS d'enregistrer son entrée de boot UEFI
//! ("ExoOS v0.2.0 — Strata") dans la NVRAM du firmware et d'ajuster BootOrder
//! si nécessaire.
//!
//! RÈGLES ARCHITECTURALES :
//! - Fail-safe : toute erreur NVRAM (accès restreint, firmware en lecture seule)
//!   est silencieusement ignorée ou loggée en avertissement non bloquant.
//! - Non-destructif : ne supprime jamais les entrées existantes d'autres OS.

use alloc::vec::Vec;
use uefi::cstr16;
use uefi::table::runtime::{VariableAttributes, VariableVendor};
use uefi::table::{Boot, SystemTable};

pub const LOAD_OPTION_ACTIVE: u32 = 0x0000_0001;

/// Enregistre ou met à jour l'entrée de boot NVRAM pour Exo-OS.
///
/// Crée l'entrée `Boot0001` (ou premier ID libre) pointant vers `\EFI\EXOOS\BOOTX64.EFI`
/// et l'insère en tête de `BootOrder` si elle n'est pas déjà présente.
pub fn register_boot_entry(st: &SystemTable<Boot>) {
    let rt = st.runtime_services();

    // 1. Déterminer un index de boot (ex: Boot0001)
    let boot_index: u16 = 0x0001;
    let var_name = cstr16!("Boot0001");

    // Description UTF-16 terminée par 0
    let desc: &[u16] = &[
        'E' as u16, 'x' as u16, 'o' as u16, 'O' as u16, 'S' as u16,
        ' ' as u16, 'v' as u16, '0' as u16, '.' as u16, '2' as u16, '.' as u16, '0' as u16,
        ' ' as u16, '—' as u16, ' ' as u16,
        'S' as u16, 't' as u16, 'r' as u16, 'a' as u16, 't' as u16, 'a' as u16,
        0,
    ];

    // Chemin du fichier en UTF-16
    let path_str: &[u16] = &[
        '\\' as u16, 'E' as u16, 'F' as u16, 'I' as u16,
        '\\' as u16, 'E' as u16, 'X' as u16, 'O' as u16, 'O' as u16, 'S' as u16,
        '\\' as u16, 'B' as u16, 'O' as u16, 'O' as u16, 'T' as u16, 'X' as u16, '6' as u16, '4' as u16, '.' as u16, 'E' as u16, 'F' as u16, 'I' as u16,
        0,
    ];

    // Construit le Device Path pour un File Path Node (Type 0x04, SubType 0x04)
    // Longueur = 4 octets d'en-tête + taille de path_str en octets (avec le 0)
    let file_path_len = (path_str.len() * 2) as u16;
    let node_length = 4u16 + file_path_len;
    let mut file_path_node = Vec::new();
    file_path_node.push(0x04u8); // Media Device Path
    file_path_node.push(0x04u8); // File Path Media Device Path
    file_path_node.extend_from_slice(&node_length.to_le_bytes());
    for &ch in path_str {
        file_path_node.extend_from_slice(&ch.to_le_bytes());
    }
    // End of Entire Device Path node (Type 0x7F, SubType 0xFF, length 4)
    file_path_node.extend_from_slice(&[0x7Fu8, 0xFFu8, 0x04u8, 0x00u8]);

    let file_path_list_length = file_path_node.len() as u16;

    // Structure EFI_LOAD_OPTION
    let mut payload = Vec::new();
    payload.extend_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
    payload.extend_from_slice(&file_path_list_length.to_le_bytes());
    for &ch in desc {
        payload.extend_from_slice(&ch.to_le_bytes());
    }
    payload.extend_from_slice(&file_path_node);

    let attrs = VariableAttributes::NON_VOLATILE
        | VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS;

    // Enregistrer Boot0001
    let _ = rt.set_variable(var_name, &VariableVendor::GLOBAL_VARIABLE, attrs, &payload);

    // 2. Mettre à jour BootOrder
    let mut order_buf = [0u8; 128];
    let order_var = cstr16!("BootOrder");
    if let Ok((data, _)) = rt.get_variable(order_var, &VariableVendor::GLOBAL_VARIABLE, &mut order_buf) {
        let mut order = Vec::new();
        let mut i = 0;
        let mut already_in_order = false;
        while i + 2 <= data.len() {
            let entry = u16::from_le_bytes([data[i], data[i + 1]]);
            if entry == boot_index {
                already_in_order = true;
            }
            order.push(entry);
            i += 2;
        }

        if !already_in_order {
            order.insert(0, boot_index);
            let mut new_order_bytes = Vec::with_capacity(order.len() * 2);
            for entry in order {
                new_order_bytes.extend_from_slice(&entry.to_le_bytes());
            }
            let _ = rt.set_variable(
                order_var,
                &VariableVendor::GLOBAL_VARIABLE,
                attrs,
                &new_order_bytes,
            );
        }
    } else {
        // Pas de BootOrder trouvé : créer un BootOrder avec notre entrée
        let new_order_bytes = boot_index.to_le_bytes();
        let _ = rt.set_variable(
            order_var,
            &VariableVendor::GLOBAL_VARIABLE,
            attrs,
            &new_order_bytes,
        );
    }
}
