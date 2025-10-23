data "aws_caller_identity" "current" {}

resource "aws_kms_key" "this" {
  description  = "Key used for encryption/decryption of WCN SOPS secrets"
  multi_region = true
}

resource "aws_kms_key_policy" "this" {
  key_id = aws_kms_key.this.id

  policy = jsonencode({
    Version = "2012-10-17"
    Id      = "key-default-1"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:root"
          AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/TerraformCloud"
        },
        Action   = "kms:*"
        Resource = "*"
      },
      {
        Effect = "Allow"
        Principal = {
          AWS = "*"
        },
        Action = [
          "kms:DescribeKey",
          "kms:Encrypt",
          "kms:Decrypt",
          "kms:ReEncrypt*",
          "kms:GenerateDataKey",
          "kms:GenerateDataKeyWithoutPlaintext"
        ],
        Resource = "*",
        "Condition" : {
          "ArnLike" : {
            "aws:PrincipalArn" : "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/AWSReservedSSO_Read-Only*"
          }
        }
      }
    ]
  })

}
