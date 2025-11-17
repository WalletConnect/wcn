#!/bin/bash
set -euxo pipefail

ECS_CLUSTER="${ecs_cluster}"

mkdir -p /etc/ecs
echo ECS_CLUSTER=$ECS_CLUSTER > /etc/ecs/ecs.config

# Make sure that EC2 instance connect is installed and running
dnf install -y ec2-instance-connect
systemctl restart sshd
