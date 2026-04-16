# sudo openconnect --useragent 'AnyConnect Darwin_x86_64 4.10.05095' vpn-ct.ecnu.edu.cn -u 20XXXXXXXXX --passwd-on-stdin --script "vpn-slice ~/ecnu-vpn-routes.txt"
SCRIPT_FILE="$HOME/Scripts/ECNU-VPN/tunnel.sh"

export SERVER="https://vpn-ct.ecnu.edu.cn"
export USERNAME="20XXXXXXXXX"
export PASSWORD="<your-password>"  # Replace with your actual password or use a secure method

echo "$PASSWORD" | \
sudo openconnect $SERVER \
    --useragent 'AnyConnect Darwin_x86_64 4.10.05095' \
    -m 1290 \
    -u $USERNAME \
    --passwd-on-stdin \
    --script "$SCRIPT_FILE" \
    -b \
    --syslog