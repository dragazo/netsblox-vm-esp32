## Peripherals

Peripherals can be added to a device by modifying the JSON-encoded peripherals config file through the remote board configuration page.


### I2C

I2C is simple communication protocol between integrated circuits.
This is a common way to connect external peripherals to embedded devices, and many of NetsBloxVM's supported peripherals require I2C.

```json
{
  "i2c": {
    "gpio_sda": <number>,
    "gpio_scl": <number>
  }
}
```

### Digital Inputs

Digital inputs are simple inputs that measure high or low levels of voltage.
These could be used to check whether a push button is pressed.

An input is read as `true` from the block-based program when it reads high voltage on the gpio pin.
The `negated` option allows you to flip this and have low voltage map to `true` instead.

```json
{
  "digitalIns": [
    {
      "name": <string>,
      "gpio": <number>,
      "negated": <bool>
    }
  ]
}
```

### Digital Outputs

Digital outputs are simple outputs that can be set to either high or low voltage.
These could be used to control simple on/off LEDs.

When an output is set to `true` from the block-based program, the voltage on the gpio pin is set to high.
The `negated` option allows you to flip this and have `true` map to high voltage instead.

```json
{
  "digitalOuts": [
    {
      "name": <string>,
      "gpio": <number>,
      "negated": <bool>
    }
  ]
}
```

### Motors

The basic motor type is a DC motor which has two gpio pins: one for powering the motor in the positive (forward) direction, and another for the negative (reverse) direction.

Motors typically take a lot of power to run, so you will need to connect this to a stronger power supply than just the NetsBloxVM embedded board supplies with its voltage out pins.
You will likely want to use a DC motor controller to power the motors (and use gpio to control it) to avoid damaging you NetsBloxVM board.

```json
{
  "motors": [
    {
      "name": <string>,
      "gpio_pos": <number>,
      "gpio_neg": <number>
    }
  ]
}
```

The above is an example of how to add motor peripherals, which can be controlled individually.
For convenience, you may want to be able to perform a single syscall to set the speed of multiple motors simultaneously.
You can use motor groups to accomplish this, which group multiple named motors into one named motor group.

```json
{
  "motor_groups": [
    {
      "name": <string>,
      "motors": [
        <string>,
        <string>,
        ...
      ]
    }
  ]
}
```

### HC-SR04

The HC-SR04 is a simple ultrasonic distance sensor.
It is controlled by two gpio pins: one to trigger an ultrasonic pulse and another to measure the echo response.

```json
{
  "hcsr04s": [
    {
      "name": <string>,
      "gpio_trigger": <number>,
      "gpio_echo": <number>
    }
  ]
}
```

### MAX30205

The MAX30205 is a human body temperature sensor, which could be made into a wearable device.
This sensor communicates over I2C, so make sure you configured I2C for the NetsBloxVM board.

```json
{
  "max30205s": [
    {
      "name": <sting>,
      "i2c_addr": <number>
    }
  ]
}
```

### IS31FL3741

The IS31FL3741 is a 13x9 RGB LED matrix made by Adafruit.
With this, you can display color images such as NetsBlox costumes/images, or manually manipulate individual pixel colors.
This sensor communicates over I2C, so make sure you configured I2C for the NetsBloxVM board.

```json
{
  "is31fl3741s": [
    {
      "name": <string>,
      "i2c_addr": <number>
    }
  ]
}
```

### BMP388

The BMP388 is a high-precision environmental sensor that measures pressure and temperature.
With this, you can display color images such as NetsBlox costumes/images, or manually manipulate individual pixel colors.
This sensor communicates over I2C, so make sure you configured I2C for the NetsBloxVM board.

```json
{
  "bmp388s": [
    {
      "name": <string>,
      "i2c_addr": <number>
    }
  ]
}
```

### LIS3DH

The LIS3DH is a 3-axis accelerometer.
With this, you can measure how quickly something accelerates (like a falling object).
Or, if the object is stationary, this lets you tell the orientation of the device by seeing the direction of gravity (down) relative to the sensor.
This sensor communicates over I2C, so make sure you configured I2C for the NetsBloxVM board.

```json
{
  "lis3dhs": [
    {
      "name": <string>,
      "i2c_addr": <number>
    }
  ]
}
```

### VEML7700

The VEML7700 is a high-precision light level sensor.
With this, you can tell if the lights are on in a room, or tell daytime vs. nighttime.
This sensor communicates over I2C, so make sure you configured I2C for the NetsBloxVM board.

```json
{
  "vaml7700s": [
    {
      "name": <string>,
      "i2c_addr": <number>
    }
  ]
}
```
