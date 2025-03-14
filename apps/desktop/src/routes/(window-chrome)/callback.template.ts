export default `
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="X-UA-Compatible" content="IE=edge">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta http-equiv="Cache-Control" content="no-store, no-cache, must-revalidate">
  <meta http-equiv="Pragma" content="no-cache">
  <title>Cap Auth</title>
  <style>
    html, body {
      width: 100%;
      height: 100%;
      margin: 0;
      padding: 0;
      font-weight: 400;
    }
    body {
      display: flex;
      align-items: center;
      justify-content: center;
      font-family: sans-serif;
      text-align: center;
      background-color: #f8f9fa;
    }
    .container {
      padding: 30px;
      width: 100%;
      max-width: 400px;
      margin: 0 auto;
    }
    .logo {
      width: 130px;
      height: auto;
      margin-bottom: 20px;
    }
    p {
      font-size: 21px;
      line-height: 26px;
      color: #12161F;
      margin: 0;
    }
    .error {
      color: #dc2626;
      margin-top: 12px;
      font-size: 16px;
    }
  </style>
</head>
<body>
  <div class="container">
    <svg class="logo" viewBox="0 0 103 40" fill="none" xmlns="http://www.w3.org/2000/svg"> <rect x="0.25" y="0.25" width="39.5" height="39.5" rx="7.75" fill="white"/> <rect x="0.25" y="0.25" width="39.5" height="39.5" rx="7.75" stroke="#E7EAF0" stroke-width="0.5"/> <path d="M20 36C28.8365 36 36 28.8365 36 20C36 11.1635 28.8365 4 20 4C11.1635 4 4 11.1635 4 20C4 28.8365 11.1635 36 20 36Z" fill="#4785FF"/> <path d="M20.0001 33C27.1797 33 33 27.1797 33 20.0001C33 12.8203 27.1797 7 20.0001 7C12.8203 7 7 12.8204 7 20.0001C7 27.1797 12.8204 33 20.0001 33Z" fill="#ADC9FF"/> <path d="M20.0001 30.0002C25.5229 30.0002 30.0002 25.5229 30.0002 20.0001C30.0002 14.4773 25.5229 10.0002 20.0001 10.0002C14.4773 10.0002 10.0002 14.4773 10.0002 20.0001C10.0002 25.5229 14.4773 30.0002 20.0001 30.0002Z" fill="white"/> <path d="M58.416 30.448C53.012 30.448 49.204 26.584 49.204 20.088C49.204 13.704 52.872 9.672 58.472 9.672C63.54 9.672 66.256 12.332 67.096 16.84L63.288 17.036C62.812 14.432 61.216 12.836 58.472 12.836C55.084 12.836 52.984 15.664 52.984 20.088C52.984 24.568 55.14 27.284 58.444 27.284C61.384 27.284 62.952 25.576 63.4 22.72L67.208 22.916C66.424 27.592 63.456 30.448 58.416 30.448ZM74.6451 30.336C71.5091 30.336 69.4371 28.852 69.4371 26.248C69.4371 23.672 71.0331 22.3 74.3091 21.656L79.2651 20.676C79.2651 18.576 78.2851 17.484 76.4091 17.484C74.6451 17.484 73.6931 18.296 73.3571 19.808L69.6891 19.64C70.2771 16.504 72.6851 14.712 76.4091 14.712C80.6651 14.712 82.8491 16.952 82.8491 20.928V26.36C82.8491 27.172 83.1291 27.396 83.6891 27.396H84.1651V30C83.9411 30.056 83.3531 30.112 82.8771 30.112C81.2531 30.112 80.0491 29.524 79.7411 27.676C79.0131 29.272 77.1091 30.336 74.6451 30.336ZM75.3731 27.732C77.7531 27.732 79.2651 26.22 79.2651 23.952V23.112L75.4011 23.896C73.8051 24.204 73.1611 24.876 73.1611 25.912C73.1611 27.088 73.9451 27.732 75.3731 27.732ZM86.8741 34.2V15.048H90.3181L90.3741 17.26C91.2421 15.608 92.8941 14.712 94.8541 14.712C99.1101 14.712 101.21 18.212 101.21 22.524C101.21 26.836 99.0821 30.336 94.8261 30.336C92.9221 30.336 91.2701 29.412 90.4581 27.956V34.2H86.8741ZM93.9861 27.424C96.1701 27.424 97.4861 25.604 97.4861 22.524C97.4861 19.444 96.1701 17.624 93.9861 17.624C91.8021 17.624 90.4581 19.276 90.4581 22.524C90.4581 25.772 91.7741 27.424 93.9861 27.424Z" fill="#12161F"/> </svg>
    <p id="message">You are now signed in. Please re-open the Cap desktop app to continue.</p>
    <div id="error-container"></div>
  </div>
</body>
</html>
`;
