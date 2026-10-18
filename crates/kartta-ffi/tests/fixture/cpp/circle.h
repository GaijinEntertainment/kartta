#pragma once

#include "shape.h"

class Circle : public Shape {
public:
    double area() const override;
};
