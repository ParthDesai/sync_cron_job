# Sync Cron - Solana Smart Contract Interaction Bot

A Rust-based cron job application that interacts with Solana smart contracts on a scheduled basis while maintaining transaction limits and state tracking with randomized distribution throughout the day.

## Features

- **Random Transaction Scheduling**: Distributes transactions randomly throughout the day for natural patterns
- **Time-Based Distribution**: Adjusts transaction probability based on time of day (higher during business hours)
- **Dynamic Daily Transaction Limits**: Configurable range of daily transaction limits with random target generation
- **User Key Pool Management**: Maintains a pool of temporary user keys with automatic expiry and rotation
- **Local KV Storage**: Uses Sled embedded database for fast, reliable local storage
- **State Management**: Tracks transaction history and status without external dependencies
- **Solana Integration**: Full Solana blockchain integration with transaction confirmation tracking
- **IPFS Integration**: Built-in Pinata IPFS support for data storage
- **Flexible Configuration**: JSON-based configuration with environment variable support
- **Comprehensive Logging**: Detailed logging for monitoring and debugging
- **Transaction Status Tracking**: Real-time monitoring of transaction confirmations

## Prerequisites

- Rust 1.70+ 
- Solana CLI tools (for keypair management)

## Installation

1. Clone the repository:
```bash
git clone <repository-url>
cd sync_cron
```

2. Build the project:
```bash
cargo build --release
```

3. Set up your configuration:
```bash
cp config.example.json config.json
# Edit config.json with your settings
```

## How Random Scheduling Works

The application uses an intelligent randomization system that:

1. **Checks every N minutes** (configurable interval) for transaction opportunities
2. **Generates daily targets** randomly within your configured range
3. **Calculates probability** based on:
   - Remaining transactions for the day
   - Time remaining in the day
   - Current time of day (business hours have higher probability)
4. **Distributes transactions naturally** throughout the day
5. **Ensures daily limits** are respected while maximizing randomness

### Time-Based Multipliers

- **Early morning (00:00-06:00)**: 30% of base probability
- **Morning (06:00-09:00)**: 70% of base probability  
- **Business hours (09:00-17:00)**: 120% of base probability
- **Evening (17:00-21:00)**: 80% of base probability
- **Late night (21:00-24:00)**: 50% of base probability

## Configuration

### JSON Configuration

Create a `config.json` file based on `config.example.json`:

```json
{
  "solana_rpc_url": "https://api.mainnet-beta.solana.com",
  "database_path": "./kv_store",
  "min_daily_transactions": 80,
  "max_daily_transactions": 120,
  "min_user_key_expiry_seconds": 3600,
  "max_user_key_expiry_seconds": 86400,
  "user_key_pool_size": 100,
  "high_rating_percentage": 0.8,
  "cron_schedule_in_minutes": 15,
  "solana_config": {
    "keypair_file": "YOUR_BASE58_PRIVATE_KEY_HERE",
    "program_id": "YOUR_PROGRAM_ID_HERE",
    "commitment": "confirmed",
    "agents": [
      "YOUR_AGENT_PRIVATE_KEY_HERE"
    ],
    "categories_supported": {
      "technology": [
        "ai",
        "blockchain",
        "software"
      ],
      "science": [
        "physics",
        "chemistry",
        "biology"
      ],
      "business": [
        "finance",
        "marketing",
        "strategy"
      ]
    }
  },
  "pinata_config": {
    "jwt_token": "your_pinata_jwt_token_here",
    "gateway_url": "https://gateway.pinata.cloud"
  }
}
```

### Environment Variables

Alternatively, configure using environment variables:

```bash
export SOLANA_RPC_URL="https://api.mainnet-beta.solana.com"
export DATABASE_PATH="./kv_store"
export MIN_DAILY_TRANSACTIONS="80"
export MAX_DAILY_TRANSACTIONS="120"
export MIN_USER_KEY_EXPIRY_SECONDS="3600"
export MAX_USER_KEY_EXPIRY_SECONDS="86400"
export USER_KEY_POOL_SIZE="100"
export HIGH_RATING_PERCENTAGE="0.8"
export CRON_SCHEDULE_IN_MINUTES="15"
export KEYPAIR_FILE="YOUR_BASE58_PRIVATE_KEY_HERE"
export PROGRAM_ID="YOUR_PROGRAM_ID"
export COMMITMENT="confirmed"
export SOLANA_AGENTS="AGENT_PRIVATE_KEY_1,AGENT_PRIVATE_KEY_2"
export PINATA_JWT_TOKEN="your_pinata_jwt_token_here"
export PINATA_GATEWAY_URL="https://gateway.pinata.cloud"
```

### Configuration Parameters

- **solana_rpc_url**: Solana RPC endpoint URL
- **database_path**: Path to local KV database directory
- **min_daily_transactions**: Minimum daily transaction target
- **max_daily_transactions**: Maximum daily transaction target
- **min_user_key_expiry_seconds**: Minimum expiry time for user keys (seconds)
- **max_user_key_expiry_seconds**: Maximum expiry time for user keys (seconds)
- **user_key_pool_size**: Number of user keys to maintain in the pool
- **high_rating_percentage**: Percentage of transactions that should have high ratings
- **cron_schedule_in_minutes**: Interval between transaction checks (minutes)
- **keypair_file**: Base58-encoded private key for main wallet
- **program_id**: Target Solana program public key
- **commitment**: Transaction commitment level (confirmed, finalized)
- **agents**: Array of Base58-encoded private keys for agent wallets
- **categories_supported**: Mapping of primary categories to their secondary categories (nested under solana_config)

## Categories Configuration

The application supports dynamic category selection for data submissions. Categories are randomly selected from the configured `categories_supported` mapping within the `solana_config` section:

### Category Structure
```json
{
  "solana_config": {
    "categories_supported": {
      "primary_category1": [
        "secondary_category1",
        "secondary_category2",
        "secondary_category3"
      ],
      "primary_category2": [
        "secondary_category21",
        "secondary_category22"
      ]
    }
  }
}
```

### Category Selection Process
1. **Random Primary Selection**: A primary category is randomly selected from the available keys
2. **Random Secondary Selection**: A secondary category is randomly selected from the chosen primary category's array
3. **Validation**: All category strings are validated against the size limits defined in the `sync_contract` crate
4. **Logging**: Selected categories are logged for each transaction

### Size Validation
- **Primary Category Size**: Must not exceed `PRIMARY_CATEGORY_SIZE` bytes (defined in sync_contract)
- **Secondary Category Size**: Must not exceed `SECONDARY_CATEGORY_SIZE` bytes (defined in sync_contract)
- **Validation Timing**: Categories are validated at application startup and configuration load

### Example Categories
The example configuration includes categories for:
- **Technology**: ai, blockchain, software, hardware, mobile
- **Science**: physics, chemistry, biology, mathematics, research
- **Business**: finance, marketing, strategy, management, consulting
- **Education**: teaching, learning, curriculum, assessment, training
- **Health**: medicine, wellness, nutrition, fitness, mental

## User Key Pool Management

The application maintains a pool of temporary user keys that are automatically generated and rotated:

### Features
- **Automatic Key Generation**: Creates new keypairs as needed
- **Expiry Management**: Keys expire after a random time within configured range
- **Pool Maintenance**: Ensures the pool always has the required number of available keys
- **Cleanup**: Automatically removes expired keys
- **Usage Tracking**: Tracks when keys are used and updates statistics

### Statistics
The application logs user key pool statistics including:
- Total keys in pool
- Available (non-expired) keys
- Expired keys awaiting cleanup

## Private Key Configuration

**⚠️ SECURITY WARNING**: Private keys provide full control over your Solana wallets. Store them securely and never commit them to version control. Consider using environment variables or secure key management systems in production.

### Extracting Private Keys from Solana Keypair Files

To convert existing Solana keypair files to Base58 private keys for configuration:

#### Method 1: Using Solana CLI (Recommended)
```bash
# For your main keypair
solana-keygen pubkey ~/.config/solana/id.json --outfile /dev/stdout

# Display the keypair in base58 format
solana-keygen pubkey ~/.config/solana/id.json --outfile /dev/stdout | tr -d '\n' | base58 -d | base58
```

#### Method 2: Using Node.js
```javascript
const fs = require('fs');
const bs58 = require('bs58');

// Read the keypair file
const keypairFile = fs.readFileSync('~/.config/solana/id.json', 'utf8');
const keypairArray = JSON.parse(keypairFile);

// Convert to base58 private key
const privateKey = bs58.encode(keypairArray);
console.log('Private key:', privateKey);
```

#### Method 3: Using Python
```python
import json
import base58

# Read the keypair file
with open('~/.config/solana/id.json', 'r') as f:
    keypair_array = json.load(f)

# Convert to base58 private key
private_key = base58.b58encode(bytes(keypair_array)).decode('utf-8')
print('Private key:', private_key)
```

### Agent Configuration

The `agents` array should contain Base58-encoded private keys for each agent wallet:

```json
{
  "agents": [
    "5J1K2N3M4P5Q6R7S8T9U0V1W2X3Y4Z5A6B7C8D9E0F1G2H3I4J5K6L7M8N9O0P1Q2R3S4T5U6V7W8X9Y0Z1A2B3C4D5E6F7G8H9I0J",
    "5K2L3M4N5O6P7Q8R9S0T1U2V3W4X5Y6Z7A8B9C0D1E2F3G4H5I6J7K8L9M0N1O2P3Q4R5S6T7U8V9W0X1Y2Z3A4B5C6D7E8F9G0H1I2J"
  ]
}
```

### Environment Variable Configuration

For agent private keys, use a comma-separated list:

```bash
export SOLANA_AGENTS="AGENT_KEY_1,AGENT_KEY_2,AGENT_KEY_3"
```

### Security Best Practices

1. **Never commit private keys** to version control
2. **Use environment variables** for sensitive data
3. **Set appropriate file permissions** (600) for config files
4. **Consider using** AWS Secrets Manager, HashiCorp Vault, or similar for production
5. **Regular key rotation** for long-running applications
6. **Monitor wallet activity** for unauthorized transactions

### Pinata IPFS Integration

This application includes built-in support for uploading data to Pinata IPFS storage:

```json
{
  "pinata_config": {
    "jwt_token": "your_pinata_jwt_token_here",
    "gateway_url": "https://gateway.pinata.cloud"
  }
}
```

Environment variables:
```bash
export PINATA_JWT_TOKEN="your_pinata_jwt_token_here"
export PINATA_GATEWAY_URL="https://gateway.pinata.cloud"
```

The PinataClient provides methods for:
- Uploading raw bytes data
- Uploading text content
- Uploading JSON data
- Getting multiple URL formats (public, gateway, IPFS)

## Usage

### Basic Usage

Run the cron job:
```bash
./target/release/sync_cron
```

Or with custom config path:
```bash
CONFIG_PATH="./my-config.json" ./target/release/sync_cron
```

### Database Management

The application automatically:
- Creates the KV database on first run (no setup required)
- Tracks all transactions with status updates
- Cleans up old records (30+ days)
- Maintains daily transaction counts and targets
- Manages user key pool with automatic expiry

### Transaction Monitoring

The application tracks:
- Transaction submission timestamps
- Transaction hashes and status
- Block confirmations and slot numbers
- Daily transaction counts and targets
- Failed transaction error messages
- Random selection decisions and probabilities
- User key pool statistics

## Local KV Database

The application uses [Sled](https://github.com/spacejam/sled) as its embedded database:

- **Zero Configuration**: No database server setup required
- **High Performance**: Fast read/write operations
- **ACID Transactions**: Reliable data consistency
- **Crash Recovery**: Automatic recovery from unexpected shutdowns
- **Small Footprint**: Minimal disk and memory usage

### Data Storage

The KV database stores:
- **Transaction Records**: `tx:{hash}` → Transaction details (JSON)
- **Daily Counters**: `daily_count:{date}` → Transaction count for the day
- **Daily Targets**: `daily_target:{date}` → Target transactions for the day
- **User Keys**: `user_key:{id}` → User key records with expiry
- **Pending Tracking**: `pending:{id}` → Transaction hash for pending transactions

### Database Location

By default, the database is stored in `./kv_store/` directory. You can change this by:
- Setting `database_path` in `config.json`
- Using the `DATABASE_PATH` environment variable

## Random Transaction Algorithm

The system uses a sophisticated probability calculation:

1. **Daily Target Generation**: Random target within configured min/max range
2. **Base Probability** = Remaining Transactions ÷ Remaining Check Cycles
3. **Time Adjustment** = Base Probability × Time Multiplier
4. **Final Probability** = Min(Time Adjustment, 0.8) but at least 0.01
5. **Random Decision** = Generate random number and compare with probability

This ensures:
- Dynamic daily targets for natural variation
- Even distribution across the day
- Natural transaction patterns
- Guaranteed completion of daily quota
- Realistic timing that mimics human behavior

## Cron Schedule Configuration

The system uses a configurable interval in minutes (default: 15 minutes). The `cron_schedule_in_minutes` parameter controls how often the system checks for transaction opportunities.

Examples:
- `15` - Every 15 minutes (default)
- `10` - Every 10 minutes (more frequent checks)
- `30` - Every 30 minutes (less frequent checks)

## Database Schema

The KV database uses the following key patterns:

### Transaction Records
```
Key: tx:{transaction_hash}
Value: {
  "id": "uuid",
  "tx_result": {
    "signatures": ["signature1", "signature2"],
    "slot": slot_number,
    "block_time": timestamp,
    "meta": transaction_meta
  },
  "sent_at": "2024-01-01T12:00:00Z",
  "status": "confirmed"
}
```

### Daily Counters
```
Key: daily_count:{YYYY-MM-DD}
Value: "transaction_count"
```

### Daily Targets
```
Key: daily_target:{YYYY-MM-DD}
Value: "target_count"
```

### User Key Records
```
Key: user_key:{user_key_id}
Value: {
  "id": "uuid",
  "pubkey": "public_key",
  "private_key": "base58_private_key",
  "created_at": "2024-01-01T12:00:00Z",
  "expires_at": "2024-01-01T12:00:00Z",
  "last_used": "2024-01-01T12:00:00Z"
}
```

## Logging

The application provides comprehensive logging:
- Random probability calculations
- Transaction submission and confirmation
- Daily target generation and tracking
- User key pool management
- Category selection for each transaction
- Time-based adjustments
- Error handling and recovery
- Database operations
- Cron schedule execution

Example log output:
```
INFO Generated new daily target for 2024-01-01: 95 (range: 80-120)
INFO User key pool stats: 100 total, 87 available, 13 expired
INFO Selected categories - Primary: 'technology', Secondary: 'blockchain'
INFO Probability calculation: 5 remaining, 32 cycles left (15 min intervals), base: 0.156, time_mult: 1.20, final: 0.188
INFO Transaction probability: 18.75%, random value: 23.45%
INFO Skipping transaction this cycle (random selection)
```

## Security Considerations

- Store private keys securely with appropriate permissions
- Use environment variables for sensitive configuration
- Monitor transaction limits to prevent unexpected costs
- Regularly backup the KV database directory for transaction history
- Random scheduling helps avoid predictable patterns
- User key pool rotation enhances transaction privacy

## Development

### Running Tests

```bash
cargo test
```

### Development Mode

```bash
cargo run
```

### Building for Production

```bash
cargo build --release
```

## Troubleshooting

### Common Issues

1. **Private key invalid**: Ensure the private key is properly Base58 encoded
2. **RPC connection failed**: Verify the Solana RPC URL is accessible
3. **Transaction failed**: Check account balances and program ID validity
4. **Database locked**: Ensure only one instance is running (Sled handles this automatically)
5. **No transactions sent**: Check probability calculations in logs
6. **User key pool errors**: Verify key generation and expiry configuration

### Debug Logging

Set the `RUST_LOG` environment variable for detailed logs:
```bash
RUST_LOG=debug ./target/release/sync_cron
```

### Database Backup

To backup your transaction history:
```bash
cp -r ./kv_store ./kv_store_backup
```

### Adjusting Randomness

To modify the randomization behavior:
- Change the check frequency by modifying `cron_schedule_in_minutes`
- Adjust time-based multipliers in `get_time_based_multiplier()`
- Modify probability caps in `calculate_transaction_probability()`
- Change daily target ranges with `min_daily_transactions` and `max_daily_transactions`

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.  