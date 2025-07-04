#!/usr/bin/env python3
"""
Utility script to extract Base58-encoded private keys from Solana keypair files.
This helps with converting existing Solana keypair files to the new private key format.

Usage:
    python extract_private_key.py ~/.config/solana/id.json
    python extract_private_key.py ~/.config/solana/agent1.json ~/.config/solana/agent2.json
"""

import json
import sys
import os
from typing import List

try:
    import base58
except ImportError:
    print("Error: base58 library is required. Install with: pip install base58")
    sys.exit(1)

def extract_private_key(keypair_path: str) -> str:
    """Extract Base58-encoded private key from a Solana keypair file."""
    try:
        # Expand user home directory
        expanded_path = os.path.expanduser(keypair_path)
        
        # Check if file exists
        if not os.path.exists(expanded_path):
            raise FileNotFoundError(f"Keypair file not found: {expanded_path}")
        
        # Read the keypair file
        with open(expanded_path, 'r') as f:
            keypair_array = json.load(f)
        
        # Validate the keypair array
        if not isinstance(keypair_array, list) or len(keypair_array) != 64:
            raise ValueError("Invalid keypair file format. Expected array of 64 bytes.")
        
        # Convert to bytes and encode as Base58
        private_key = base58.b58encode(bytes(keypair_array)).decode('utf-8')
        
        return private_key
    
    except Exception as e:
        print(f"Error processing {keypair_path}: {e}")
        return None

def main():
    """Main function to process command line arguments."""
    if len(sys.argv) < 2:
        print("Usage: python extract_private_key.py <keypair_file1> [keypair_file2] ...")
        print("Example: python extract_private_key.py ~/.config/solana/id.json")
        sys.exit(1)
    
    keypair_files = sys.argv[1:]
    private_keys = []
    
    print("🔑 Extracting private keys from Solana keypair files...")
    print("=" * 60)
    
    for i, keypair_file in enumerate(keypair_files, 1):
        print(f"\nProcessing file {i}: {keypair_file}")
        
        private_key = extract_private_key(keypair_file)
        if private_key:
            private_keys.append(private_key)
            print(f"✅ Success!")
            print(f"Private key: {private_key}")
        else:
            print(f"❌ Failed to extract private key")
    
    # Print configuration examples
    if private_keys:
        print("\n" + "=" * 60)
        print("📋 CONFIGURATION EXAMPLES")
        print("=" * 60)
        
        if len(private_keys) == 1:
            print("\n🔧 For main keypair (config.json):")
            print(f'  "keypair_private_key": "{private_keys[0]}"')
            
            print("\n🔧 For environment variable:")
            print(f'  export KEYPAIR_PRIVATE_KEY="{private_keys[0]}"')
        
        if len(private_keys) > 1:
            print("\n🔧 For agents array (config.json):")
            print('  "agents": [')
            for i, key in enumerate(private_keys):
                comma = "," if i < len(private_keys) - 1 else ""
                print(f'    "{key}"{comma}')
            print('  ]')
            
            print("\n🔧 For environment variable:")
            agents_env = ",".join(private_keys)
            print(f'  export SOLANA_AGENTS="{agents_env}"')
        
        print("\n" + "=" * 60)
        print("⚠️  SECURITY WARNING")
        print("=" * 60)
        print("• Never commit private keys to version control")
        print("• Store them securely using environment variables")
        print("• Set appropriate file permissions (600) for config files")
        print("• Consider using secure key management systems in production")
        print("• Monitor wallet activity for unauthorized transactions")

if __name__ == "__main__":
    main() 