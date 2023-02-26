set -e

mkdir -p cert
cd cert

IP="${1:-127.0.0.1}"

echo "Creating certification files for $IP"

echo
echo "Generating san.txt"
echo "subjectAltName = DNS:$IP, IP:$IP" > san.txt
echo "basicConstraints=CA:TRUE" >> san.txt

echo
echo "Generating server.key and server.csr"
# openssl genrsa 2048 > server.key
MSYS_NO_PATHCONV=1 openssl req -new -newkey rsa:4096 -nodes \
  -keyout server.key -out server.csr \
  -subj "/C=JP/ST=Tokyo/O=Example Organization/CN=$IP"

echo
echo "Generating server.crt"
openssl x509 -days 3650 -req -signkey server.key -in server.csr -out server.crt -extfile san.txt

echo
echo "Generating server.pfx"
openssl pkcs12 -export -inkey server.key -in server.crt -out server.pfx -passout pass: