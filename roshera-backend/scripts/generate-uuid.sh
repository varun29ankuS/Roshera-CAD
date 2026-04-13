#!/bin/bash
# Simple UUID v4 generator for systems without uuidgen
python3 -c "import uuid; print(str(uuid.uuid4()))" 2>/dev/null || \
python -c "import uuid; print(str(uuid.uuid4()))" 2>/dev/null || \
echo "550e8400-e29b-41d4-a716-446655440000"
