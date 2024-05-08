#!/bin/bash

# Specify the file to upload and the S3 bucket name
FILE_TO_UPLOAD=".build/"
S3_BUCKET="sp1-circuits"

# Check for unstaged changes in the Git repository
if ! git diff --quiet; then
    echo "Error: There are unstaged changes. Please commit or stash them before running this script."
    exit 1
fi

# Get the current git commit hash (shorthand)
COMMIT_HASH=$(git rev-parse --short HEAD)
if [ $? -ne 0 ]; then
    echo "Failed to retrieve Git commit hash."
    exit 1
fi

# Create archive named after the commit hash
ARCHIVE_NAME="${COMMIT_HASH}.tar.gz"
tar -czvf $ARCHIVE_NAME $FILE_TO_UPLOAD
if [ $? -ne 0 ]; then
    echo "Failed to create archive."
    exit 1
fi

# Upload the file to S3, naming it after the current commit hash
aws s3 cp "$FILE_TO_UPLOAD" "s3://$S3_BUCKET/$ARCHIVE_NAME"
if [ $? -ne 0 ]; then
    echo "Failed to upload file to S3."
    exit 1
fi

echo "Succesfully uploaded build artifacts to s3://$S3_BUCKET/$ARCHIVE_NAME"