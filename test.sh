set -e
alias curl="curl -f"

BASEURL="http://localhost:5666/api/0"

URL="$BASEURL/buckets/testid"
printf "Create bucket: "
curl $URL -X POST -d '{"id": "testid", "type": "testtype", "client": "testclient", "hostname": "testhost"}' -H "Content-Type: application/json"

printf "\nBucket info\n"
URL="$BASEURL/buckets/testid/"
curl $URL -X GET
printf "\nBucket info of non-existent bucket\n"
URL="$BASEURL/buckets/testid2/"
set +e
curl $URL -X GET || :
set -e
printf "\nEvent count\n"
URL="$BASEURL/buckets/testid/events/count"
curl $URL -X GET
printf "\nInsert event\n"
URL="$BASEURL/buckets/testid/events"
DATE=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
DATA='[{"timestamp": "'$DATE'", "duration": 1.1, "data": {"key": "value"}}]'
echo $DATA
curl $URL -X POST -d "$DATA" -H "Content-Type: application/json"
printf "\nHeartbeat\n"
URL="$BASEURL/buckets/testid/heartbeat?pulsetime=1"
DATA='{"timestamp": "'$DATE'", "duration": 3.0, "data": {"key": "value"}}'
curl $URL -X POST -d "$DATA" -H "Content-Type: application/json"

# TODO: Needs to be fixed!
#printf "\nEvents\n"
#URL="http://localhost:5666/buckets/testid/events"
#curl $URL -X GET
printf "\nEvents\n"
URL="$BASEURL/buckets/testid/events?"
curl $URL -X GET
printf "\nEvents\n"
URL="$BASEURL/buckets/testid/events?start=2000-01-01T00:00:00Z&end=2030-01-01T00:00:00Z&limit=-1"
curl $URL -X GET

printf "\n"
