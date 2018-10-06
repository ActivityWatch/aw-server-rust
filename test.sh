set -e
alias curl="curl -f"

# User
URL="http://localhost:5666/api/0/buckets/testid"
printf "Create bucket: "
curl $URL -X POST -d '{"id": "testid", "type": "testtype", "client": "testclient", "hostname": "testhost"}' -H "Content-Type: application/json"

printf "\nBucket info\n"
URL="http://localhost:8000/api/0/buckets/testid/"
curl $URL -X GET
printf "\nEvent count\n"
URL="http://localhost:8000/api/0/buckets/testid/events/count"
curl $URL -X GET
printf "\nInsert event\n"
URL="http://localhost:8000/api/0/buckets/testid/events"
DATE=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
DATA='[{"timestamp": "'$DATE'", "duration": 1.1, "data": {"key": "value"}}]'
echo $DATA
curl $URL -X POST -d "$DATA" -H "Content-Type: application/json"
printf "\nHeartbeat\n"
URL="http://localhost:8000/api/0/buckets/testid/events/heartbeat?pulsetime=1"
DATA='{"timestamp": "'$DATE'", "duration": 3.0, "data": {"key": "value"}}'
curl $URL -X POST -d "$DATA" -H "Content-Type: application/json"

# TODO: Needs to be fixed!
#printf "\nEvents\n"
#URL="http://localhost:8000/api/0/buckets/testid/events"
#curl $URL -X GET
printf "\nEvents\n"
URL="http://localhost:8000/api/0/buckets/testid/events?"
curl $URL -X GET
printf "\nEvents\n"
URL="http://localhost:8000/api/0/buckets/testid/events?start=2000-01-01T00:00:00Z&end=2030-01-01T00:00:00Z&limit=-1"
curl $URL -X GET

printf "\n"
