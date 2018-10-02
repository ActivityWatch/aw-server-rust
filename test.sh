set -e
alias curl="curl"

# User
URL="http://localhost:8000/api/0/buckets/testid"
printf "Create bucket: "
curl $URL -X POST -d '{"id": "testid", "type": "testtype", "client": "testclient", "hostname": "testhost"}' -H "Content-Type: application/json"

printf "\nBucket info\n"
URL="http://localhost:8000/api/0/buckets/testid/" #?start=2018-09-29T08:38:10+02:00&end=2018-09-29T09:38:10+02:00&limit=-1"
curl $URL -X GET
printf "\nEvent count\n"
URL="http://localhost:8000/api/0/buckets/testid/events/count" #?start=2018-09-29T08:38:10+02:00&end=2018-09-29T09:38:10+02:00&limit=-1"
curl $URL -X GET
printf "\nInsert event\n"
URL="http://localhost:8000/api/0/buckets/testid/events" #?start=2018-09-29T08:38:10+02:00&end=2018-09-29T09:38:10+02:00&limit=-1"
#DATA='[{"timestamp": "'$(date --rfc-3339=seconds)'", "duration": 1.0, "data": {"key": "value"}}]'
DATE=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
DATA='[{"timestamp": "'$DATE'", "duration": 1.1, "data": {"key": "value"}}]'
echo $DATA
curl $URL -X POST -d "$DATA" -H "Content-Type: application/json"
printf "\nEvents\n"
URL="http://localhost:8000/api/0/buckets/testid/events" #?start=2018-09-29T08:38:10+02:00&end=2018-09-29T09:38:10+02:00&limit=-1"
curl $URL -X GET
printf "\nEvents\n"
URL="http://localhost:8000/api/0/buckets/testid/events?" #?start=2018-09-29T08:38:10+02:00&end=2018-09-29T09:38:10+02:00&limit=-1"
curl $URL -X GET
printf "\nEvents\n"
URL="http://localhost:8000/api/0/buckets/testid/events?start=2000-01-01T00:00:00Z&end=2030-01-01T00:00:00Z&limit=-1"
curl $URL -X GET
