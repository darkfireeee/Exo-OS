//! Migration Assistant
//!
//! Helps migrate applications to Exo-OS

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Migration plan for an application
#[derive(Debug, Clone)]
pub struct MigrationPlan {
    /// Application name
    pub app_name: String,
    /// Migration steps
    pub steps: Vec<MigrationStep>,
    /// Estimated difficulty (1-10)
    pub difficulty: u8,
    /// Estimated time (hours)
    pub estimated_time_hours: u32,
    /// Required kernel features
    pub required_features: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MigrationStep {
    /// Step number
    pub number: usize,
    /// Description
    pub description: String,
    /// Category
    pub category: StepCategory,
    /// Automated (can be done automatically)
    pub automated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepCategory {
    Analysis,
    SourceModification,
    BuildConfiguration,
    Testing,
    Deployment,
}

/// Migration assistant
pub struct MigrationAssistant {
    /// Known migration patterns
    patterns: BTreeMap<String, MigrationPattern>,
}

#[derive(Debug, Clone)]
struct MigrationPattern {
    name: String,
    detection_keywords: Vec<String>,
    steps: Vec<String>,
}

impl MigrationAssistant {
    pub fn new() -> Self {
        let mut patterns = BTreeMap::new();

        // Common migration patterns
        patterns.insert(
            "pthread".to_string(),
            MigrationPattern {
                name: "POSIX Threads".to_string(),
                detection_keywords: vec!["pthread_create".to_string(), "pthread_join".to_string()],
                steps: vec![
                    "Verify pthread API usage".to_string(),
                    "Test thread synchronization primitives".to_string(),
                    "Check TLS (Thread Local Storage) usage".to_string(),
                ],
            },
        );

        patterns.insert(
            "networking".to_string(),
            MigrationPattern {
                name: "Network Sockets".to_string(),
                detection_keywords: vec![
                    "socket".to_string(),
                    "bind".to_string(),
                    "listen".to_string(),
                ],
                steps: vec![
                    "Verify socket API calls".to_string(),
                    "Test connection establishment".to_string(),
                    "Validate data transfer".to_string(),
                ],
            },
        );

        Self { patterns }
    }

    /// Create migration plan for an application
    pub fn create_plan(&self, app_info: &ApplicationInfo) -> MigrationPlan {
        let mut steps = Vec::new();
        let mut difficulty = 1u8;

        // Step 1: Analysis
        steps.push(MigrationStep {
            number: 1,
            description: "Analyze application dependencies".to_string(),
            category: StepCategory::Analysis,
            automated: true,
        });

        steps.push(MigrationStep {
            number: 2,
            description: "Identify required syscalls".to_string(),
            category: StepCategory::Analysis,
            automated: true,
        });

        // Detect patterns
        let mut features = Vec::new();
        for pattern in self.patterns.values() {
            if app_info.source_contains_keywords(&pattern.detection_keywords) {
                difficulty += 1;
                features.push(pattern.name.clone());

                for (_i, step) in pattern.steps.iter().enumerate() {
                    steps.push(MigrationStep {
                        number: steps.len() + 1,
                        description: step.clone(),
                        category: StepCategory::Testing,
                        automated: false,
                    });
                }
            }
        }

        // Build steps
        steps.push(MigrationStep {
            number: steps.len() + 1,
            description: "Configure build system for Exo-OS".to_string(),
            category: StepCategory::BuildConfiguration,
            automated: false,
        });

        steps.push(MigrationStep {
            number: steps.len() + 1,
            description: "Compile application".to_string(),
            category: StepCategory::BuildConfiguration,
            automated: true,
        });

        // Test steps
        steps.push(MigrationStep {
            number: steps.len() + 1,
            description: "Run unit tests".to_string(),
            category: StepCategory::Testing,
            automated: true,
        });

        steps.push(MigrationStep {
            number: steps.len() + 1,
            description: "Run integration tests".to_string(),
            category: StepCategory::Testing,
            automated: false,
        });

        // Deploy
        steps.push(MigrationStep {
            number: steps.len() + 1,
            description: "Deploy to Exo-OS".to_string(),
            category: StepCategory::Deployment,
            automated: false,
        });

        MigrationPlan {
            app_name: app_info.name.clone(),
            steps,
            difficulty: difficulty.min(10),
            estimated_time_hours: (difficulty as u32) * 4,
            required_features: features,
        }
    }

    /// Generate migration guide
    pub fn generate_guide(&self, plan: &MigrationPlan) -> String {
        use alloc::format;

        let mut guide = String::new();

        guide.push_str(&format!("=== Migration Guide: {} ===\n\n", plan.app_name));
        guide.push_str(&format!("Difficulty: {}/10\n", plan.difficulty));
        guide.push_str(&format!(
            "Estimated Time: {} hours\n\n",
            plan.estimated_time_hours
        ));

        if !plan.required_features.is_empty() {
            guide.push_str("Required Features:\n");
            for feature in &plan.required_features {
                guide.push_str(&format!("  - {}\n", feature));
            }
            guide.push_str("\n");
        }

        guide.push_str("Migration Steps:\n\n");
        for step in &plan.steps {
            let auto_marker = if step.automated { "🤖" } else { "👤" };
            guide.push_str(&format!(
                "{}. {} {:?}\n   {}\n\n",
                step.number, auto_marker, step.category, step.description
            ));
        }

        guide
    }

    /// Suggest optimizations
    pub fn suggest_optimizations(&self, _app_info: &ApplicationInfo) -> Vec<String> {
        vec![
            "Consider using Exo-OS native IPC for better performance".to_string(),
            "Use zero-copy operations for large data transfers".to_string(),
            "Enable syscall batching for I/O-intensive operations".to_string(),
        ]
    }
}

/// Application information for migration
pub struct ApplicationInfo {
    pub name: String,
    pub source_files: Vec<String>,
    pub dependencies: Vec<String>,
}

impl ApplicationInfo {
    fn source_contains_keywords(&self, keywords: &[String]) -> bool {
        // For now, just check if any dependency matches
        keywords
            .iter()
            .any(|kw| self.dependencies.iter().any(|dep| dep.contains(kw)))
    }
}

/// Create a migration plan
pub fn create_migration_plan(app_info: &ApplicationInfo) -> MigrationPlan {
    let assistant = MigrationAssistant::new();
    assistant.create_plan(app_info)
}
