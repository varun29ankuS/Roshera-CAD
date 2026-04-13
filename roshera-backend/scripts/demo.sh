#!/bin/bash
set -e

echo "🎯 Running Roshera CAD Demo"
echo "==========================="

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Check if server is running
check_server() {
    if ! curl -s http://localhost:8080/health >/dev/null 2>&1; then
        echo -e "${YELLOW}Starting server...${NC}"
        cargo build --release
        cargo run --release --bin api-server &
        SERVER_PID=$!
        
        # Wait for server to start
        echo -n "Waiting for server to start"
        for i in {1..30}; do
            if curl -s http://localhost:8080/health >/dev/null 2>&1; then
                echo -e " ${GREEN}✓${NC}"
                break
            fi
            echo -n "."
            sleep 1
        done
        
        if ! curl -s http://localhost:8080/health >/dev/null 2>&1; then
            echo -e " ${RED}✗${NC}"
            echo "Failed to start server"
            exit 1
        fi
    else
        echo -e "${GREEN}✓ Server already running${NC}"
    fi
}

# Demo function
run_demo() {
    local description=$1
    local method=$2
    local endpoint=$3
    local data=$4
    
    echo -e "\n${BLUE}Demo: $description${NC}"
    echo "Request:"
    echo "  $method $endpoint"
    if [ -n "$data" ]; then
        echo "  Body: $data" | jq .
    fi
    
    echo -e "\nResponse:"
    
    if [ "$method" = "GET" ]; then
        curl -s -X GET "http://localhost:8080$endpoint" | jq .
    else
        curl -s -X POST "http://localhost:8080$endpoint" \
            -H "Content-Type: application/json" \
            -d "$data" | jq .
    fi
    
    echo -e "${GREEN}✓ Complete${NC}"
    sleep 1
}

# Main demo flow
check_server

echo -e "\n${YELLOW}Starting demo sequence...${NC}"

# 1. Health check
run_demo "Health Check" "GET" "/health" ""

# 2. Create session
SESSION_RESPONSE=$(curl -s -X POST http://localhost:8080/api/sessions)
SESSION_ID=$(echo $SESSION_RESPONSE | jq -r '.id')
echo -e "\n${GREEN}Created session: $SESSION_ID${NC}"

# 3. Create geometry primitives
run_demo "Create a Box" "POST" "/api/geometry" '{
    "shape_type": "Box",
    "width": 10.0,
    "height": 5.0,
    "depth": 3.0,
    "position": [0.0, 0.0, 0.0],
    "material": "steel"
}'

run_demo "Create a Sphere" "POST" "/api/geometry" '{
    "shape_type": "Sphere",
    "radius": 2.5,
    "position": [5.0, 0.0, 0.0],
    "material": "plastic"
}'

run_demo "Create a Gear" "POST" "/api/geometry" '{
    "shape_type": "Gear",
    "teeth": 12.0,
    "diameter": 50.0,
    "thickness": 5.0,
    "position": [-5.0, 0.0, 0.0]
}'

# 4. Boolean operation
run_demo "Boolean Union" "POST" "/api/boolean" '{
    "operation": "Union",
    "objects": ["00000000-0000-0000-0000-000000000001", "00000000-0000-0000-0000-000000000002"]
}'

# 5. Natural language command
run_demo "AI Command: Create a cylinder" "POST" "/api/ai/command" '{
    "command": "create a cylinder with radius 1.5 and height 4",
    "session_id": "'$SESSION_ID'"
}'

run_demo "AI Command: Create and union" "POST" "/api/ai/command" '{
    "command": "create a box and sphere then union them together",
    "session_id": "'$SESSION_ID'"
}'

# 6. Export
run_demo "Export to STL" "POST" "/api/export" '{
    "format": "STL",
    "objects": ["00000000-0000-0000-0000-000000000001"]
}'

# 7. Performance test
echo -e "\n${YELLOW}Running performance test...${NC}"
echo "Creating 10 objects rapidly..."

START_TIME=$(date +%s.%N)
for i in {1..10}; do
    curl -s -X POST http://localhost:8080/api/geometry \
        -H "Content-Type: application/json" \
        -d '{
            "shape_type": "Sphere",
            "radius": '"$i"'.0,
            "position": ['"$i"'.0, 0.0, 0.0]
        }' >/dev/null
done
END_TIME=$(date +%s.%N)

ELAPSED=$(echo "$END_TIME - $START_TIME" | bc)
echo -e "${GREEN}✓ Created 10 objects in ${ELAPSED}s${NC}"

# 8. List sessions
run_demo "List All Sessions" "GET" "/api/sessions" ""

# Cleanup
echo -e "\n${GREEN}✅ Demo complete!${NC}"

if [ -n "$SERVER_PID" ]; then
    echo -e "\n${YELLOW}Stopping server...${NC}"
    kill $SERVER_PID 2>/dev/null || true
fi

echo -e "\n${BLUE}Summary:${NC}"
echo "- Created various geometry primitives"
echo "- Performed boolean operations"
echo "- Tested natural language commands"
echo "- Exported geometry to STL format"
echo "- Verified performance with rapid creation"
echo ""
echo "Check the ./exports directory for exported files!"
