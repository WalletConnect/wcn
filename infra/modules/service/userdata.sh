#!/bin/bash
set -euxo pipefail

ECS_CLUSTER="${ecs_cluster}"
EBS_DEVICE_PATH="${ebs_device_path}"
MOUNT_POINT="${mount_point}"

mkdir -p /etc/ecs
echo ECS_CLUSTER=$ECS_CLUSTER > /etc/ecs/ecs.config

if [[ $EBS_DEVICE_PATH ]]; then
  # Wait for EBS device to appear
  for i in $(seq 1 30); do
    [[ -b $EBS_DEVICE_PATH ]] && break
    sleep 2
  done

  FSTYPE="$(blkid -s TYPE -o value $EBS_DEVICE_PATH || true)"
  if [[ -z $FSTYPE ]]; then
    # Make XFS file system
    mkfs.xfs $EBS_DEVICE_PATH
  fi

  # Make the drive to automount on restarts
  EBS_DEVICE_UUID="$(blkid -s UUID -o value $EBS_DEVICE_PATH)"
  echo "UUID=$EBS_DEVICE_UUID $MOUNT_POINT xfs defaults,nofail 0 2" >> /etc/fstab

  mkdir -p $MOUNT_POINT
  mount -a

  chown -R 1001:1001 $MOUNT_POINT
fi

# Make sure that EC2 instance connect is installed and running
dnf install -y ec2-instance-connect
systemctl restart sshd

# invalidate
