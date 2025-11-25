//! RTC (Real-Time Clock) driver
//! 
//! Provides access to CMOS RTC for date/time

use core::arch::asm;

/// RTC I/O ports
const RTC_ADDRESS: u16 = 0x70;
const RTC_DATA: u16 = 0x71;

/// RTC registers
const RTC_SECONDS: u8 = 0x00;
const RTC_MINUTES: u8 = 0x02;
const RTC_HOURS: u8 = 0x04;
const RTC_DAY: u8 = 0x07;
const RTC_MONTH: u8 = 0x08;
const RTC_YEAR: u8 = 0x09;
const RTC_STATUS_A: u8 = 0x0A;
const RTC_STATUS_B: u8 = 0x0B;

/// Date and time structure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Convert to UNIX timestamp (seconds since 1970-01-01)
    pub fn to_unix_timestamp(&self) -> u64 {
        // Simplified calculation (doesn't account for leap seconds)
        let mut year = self.year as u64;
        let month = self.month as u64;
        let day = self.day as u64;
        
        // Days since epoch
        let mut days = 0u64;
        
        // Add days for complete years
        for y in 1970..year {
            if is_leap_year(y as u16) {
                days += 366;
            } else {
                days += 365;
            }
        }
        
        // Add days for complete months in current year
        let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for m in 1..month {
            let mut month_days = days_in_month[(m - 1) as usize] as u64;
            if m == 2 && is_leap_year(year as u16) {
                month_days = 29;
            }
            days += month_days;
        }
        
        // Add days in current month
        days += day - 1;
        
        // Convert to seconds
        let seconds = days * 86400
            + (self.hour as u64) * 3600
            + (self.minute as u64) * 60
            + (self.second as u64);
        
        seconds
    }
}

/// Check if year is leap year
fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// RTC structure
pub struct Rtc;

impl Rtc {
    /// Read RTC register
    fn read_register(reg: u8) -> u8 {
        unsafe {
            // Select register
            outb(RTC_ADDRESS, reg);
            // Read data
            inb(RTC_DATA)
        }
    }
    
    /// Write RTC register
    fn write_register(reg: u8, value: u8) {
        unsafe {
            outb(RTC_ADDRESS, reg);
            outb(RTC_DATA, value);
        }
    }
    
    /// Check if RTC update is in progress
    fn is_updating() -> bool {
        Self::read_register(RTC_STATUS_A) & 0x80 != 0
    }
    
    /// Wait for RTC update to complete
    fn wait_for_update() {
        while Self::is_updating() {
            core::hint::spin_loop();
        }
    }
    
    /// Read current date and time
    pub fn read() -> Option<DateTime> {
        // Wait for update to complete
        Self::wait_for_update();
        
        // Read values
        let second = Self::read_register(RTC_SECONDS);
        let minute = Self::read_register(RTC_MINUTES);
        let hour = Self::read_register(RTC_HOURS);
        let day = Self::read_register(RTC_DAY);
        let month = Self::read_register(RTC_MONTH);
        let year = Self::read_register(RTC_YEAR);
        
        // Check format (BCD vs binary)
        let status_b = Self::read_register(RTC_STATUS_B);
        let is_bcd = (status_b & 0x04) == 0;
        
        // Convert BCD to binary if needed
        let second = if is_bcd { bcd_to_binary(second) } else { second };
        let minute = if is_bcd { bcd_to_binary(minute) } else { minute };
        let hour = if is_bcd { bcd_to_binary(hour & 0x7F) } else { hour };
        let day = if is_bcd { bcd_to_binary(day) } else { day };
        let month = if is_bcd { bcd_to_binary(month) } else { month };
        let year = if is_bcd { bcd_to_binary(year) } else { year };
        
        // Convert 2-digit year to full year (assume 2000-2099)
        let full_year = 2000 + year as u16;
        
        Some(DateTime {
            year: full_year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

/// Convert BCD to binary
fn bcd_to_binary(bcd: u8) -> u8 {
    ((bcd >> 4) * 10) + (bcd & 0x0F)
}

/// Read RTC (shorthand)
pub fn read_rtc() -> Option<DateTime> {
    Rtc::read()
}

/// Initialize RTC
pub fn init() {
    // Enable RTC interrupts (optional)
    // For now, just read to verify it works
    let _ = read_rtc();
}

/// Output byte to I/O port
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nostack, nomem)
    );
}

/// Input byte from I/O port
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nostack, nomem)
    );
    value
}
