
#[tokio::main]
async fn main() {
    println!("Hey, sleepin");
    tokio::time::delay_for(std::time::Duration::from_secs(60)).await;
    println!("Done sleepin");
}

// Constantly load configuration
// Based on configuration, spawn things
// Hold onto pointer to things so we can change based on configuration
