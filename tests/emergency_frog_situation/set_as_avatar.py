import frog

X,Y = 50, 50
W = 100
H = 100


class Toad(frog.Frog):
    def __init__(self, x, y, vx, vy):
        super().__init__(x, y, vx, vy)
        self.name = "Bob"


if __name__ == "__main__":
    toad = Toad(100, 100, 200, -200)
    toad.jump(W, H)
    print(toad.name, toad.x, toad.y)

