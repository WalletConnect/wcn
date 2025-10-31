#!/bin/bash
set -euxo pipefail

ECS_CLUSTER="${ecs_cluster}"
EBS_DEVICE_PATH="${ebs_device_path}"
MOUNT_POINT="${mount_point}"

mkdir -p /etc/ecs
echo ECS_CLUSTER=${ecs_cluster} > /etc/ecs/ecs.config

# Wait for EBS device to appear
for i in $(seq 1 30); do
  [[ -b "${EBS_DEVICE_PATH}" ]] && break
  sleep 2
done

# Make XFS file system
mkfs.xfs $EBS_DEVICE_PATH

# Make the drive to automount on restarts
EBS_DEVICE_UUID="$(blkid -s UUID -o value "${EBS_DEVICE_PATH}")"
echo "UUID=${EBS_DEVICE_UUID} ${MOUNT_POINT} xfs defaults,nofail 0 2" >> /etc/fstab

mount -a

chown 1001:1001 $MOUNT_POINT
