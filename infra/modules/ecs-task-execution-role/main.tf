variable "name_prefix" {
  type = string
}

variable "ssm_parameter_arns" {
  type = list(string)
}

resource "aws_iam_role" "this" {
  name_prefix = var.name_prefix
  assume_role_policy = jsonencode({
    Version   = "2012-10-17",
    Statement = [{ Effect = "Allow", Principal = { Service = "ecs-tasks.amazonaws.com" }, Action = "sts:AssumeRole" }]
  })
}

resource "aws_iam_role_policy_attachment" "AmazonECSTaskExecutionRolePolicy" {
  role       = aws_iam_role.this.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "ssm" {
  name = "ecs-exec-ssm-kms"
  role = aws_iam_role.exec.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      { Effect = "Allow", Action = ["ssm:GetParameter", "ssm:GetParameters", "ssm:GetParametersByPath"], Resource = var.ssm_parameter_arns },
      { Effect = "Allow", Action = ["kms:Decrypt"], Resource = "*" }
    ]
  })
}

output "arn" {
  value = aws_iam_role.this.arn
}
