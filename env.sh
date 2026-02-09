secrets=$(sops -d $1)

smart_contract_address=$(echo $secrets | jq -r '.["smart_contract_address_unencrypted"]')
smart_contract_encryption_key=$(echo $secrets | jq -r '.["smart_contract_encryption_key"]')
ecdsa_private_key=$(echo $secrets | jq -r '.["ecdsa_private_key"]')
kms_key_arn=$(echo $secrets | jq -r '.["kms_key_arn_unencrypted"]')
rpc_provider_url=$(echo $secrets | jq -r '.["rpc_provider_url"]')

export WCN_CLUSTER_SMART_CONTRACT_OWNER_KMS_KEY_ARN=$kms_key_arn
export WCN_CLUSTER_SMART_CONTRACT_ADDRESS=$smart_contract_address
export WCN_CLUSTER_SMART_CONTRACT_ENCRYPTION_KEY=$smart_contract_encryption_key
export WCN_NODE_OPERATOR_PRIVATE_KEY=$ecdsa_private_key
export OPTIMISM_RPC_PROVIDER_URL=$rpc_provider_url
